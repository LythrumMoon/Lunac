// core-agent/src/peers.rs
//! 子代理通信通道（A13「多代理通信 `SendMessage` / `ListPeers`」，2026-10-05）。
//!
//! **为什么只有「同批兄弟」这一种形态**：本仓子代理是「同一轮里并发跑、跑完各自
//! 回报告」（`main.rs` 的 `BatchKind::Subagent` + `thread::scope`），主循环在整批
//! 跑完之前**一直阻塞** ⇒「主代理给正在跑的代理发消息」在当前架构下根本没有窗口。
//! 唯一真实存在的窗口 = 同一批里并发的那几个子代理彼此之间 —— 这正是 A13 里那句
//! 「A14 的子代理之间**没有任何通道**」要补的洞。所以本模块只做这一件事：让同批
//! 兄弟能互相看见（`ListPeers`）与投递一段文本（`SendMessage`）。
//!
//! **不违反 §11 规则 54**：通道只存在于**兄弟子代理之间**，既不把主对话历史拼进
//! 子代理，也不把子代理的中间过程回灌主对话；主代理调 `ListPeers` 只会看到「谁在跑」，
//! 调 `SendMessage` 时因没有存活 peer 而如实报错（它被阻塞，发不出去）。
//!
//! 生命周期：`run_agent_tool` 在 `task_started` 之后注册、返回前注销（另有 RAII 兜底，
//! 防子代理线程 panic 时留下僵尸）；`run_subagent` 每轮开始 drain 自己的收件箱并
//! 追加成一条 `user` 消息。

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// 单条消息的字符上限（收件方会把它当一条 `user` 消息读进自己的 history ⇒ 必须有硬上限，
/// 否则兄弟可以拿它把对方的预算刷爆，见 §11 规则 59 同一条纪律）。
pub const MAX_MAIL_CHARS: usize = 2000;
/// 每个 peer 未读收件箱的条数上限。满了就**拒绝发送**（回错误给发送方），不静默丢弃 ——
/// 静默丢等于「发了但对方永远收不到」，是最难排查的一类失效。
pub const MAX_INBOX: usize = 16;

struct Peer {
    task_id: String,
    description: String,
}

struct Mail {
    from: String,
    content: String,
}

#[derive(Default)]
struct Inner {
    /// 存活 peer，按注册顺序（= 同一批里 `tool_use` 原顺序的子集）
    peers: Vec<Peer>,
    /// task_id → 未读消息
    inboxes: HashMap<String, Vec<Mail>>,
}

/// 进程内的 peer 登记处 + 收件箱。线程安全；同一时刻只有一个实例（[`bus`]）。
#[derive(Default)]
pub struct PeerBus {
    inner: Mutex<Inner>,
}

impl PeerBus {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        // 毒化恢复：登记表只是内存态协调数据，某个线程 panic 不该让整场对话再也发不出消息。
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// 登记一个存活 peer（`Agent` 子代理启动时）。同 id 重复登记只更新描述。
    pub fn register(&self, task_id: &str, description: &str) {
        let mut g = self.lock();
        if let Some(p) = g.peers.iter_mut().find(|p| p.task_id == task_id) {
            p.description = description.to_string();
        } else {
            g.peers.push(Peer {
                task_id: task_id.to_string(),
                description: description.to_string(),
            });
        }
    }

    /// 注销一个 peer（子代理结束时），连它的收件箱一起清掉。
    pub fn unregister(&self, task_id: &str) {
        let mut g = self.lock();
        g.peers.retain(|p| p.task_id != task_id);
        g.inboxes.remove(task_id);
    }

    /// 当前存活的 peer 快照：`(task_id, description)`，按注册顺序。
    pub fn list(&self) -> Vec<(String, String)> {
        let g = self.lock();
        g.peers
            .iter()
            .map(|p| (p.task_id.clone(), p.description.clone()))
            .collect()
    }

    /// 把一段文本投给 `to`。`to` 必须是**当前存活**的 peer（拿 [`list`](Self::list) 的
    /// task_id）—— 已经跑完的代理收不到，如实报错，不静默丢进一个没人读的收件箱。
    pub fn send(&self, from: &str, to: &str, content: &str) -> Result<usize, String> {
        let content = content.trim();
        if content.is_empty() {
            return Err("缺少 content 参数：要发的内容不能为空".into());
        }
        let mut g = self.lock();
        if !g.peers.iter().any(|p| p.task_id == to) {
            let alive: Vec<&str> = g.peers.iter().map(|p| p.task_id.as_str()).collect();
            return Err(if alive.is_empty() {
                "当前没有正在运行的代理可以发消息".to_string()
            } else {
                format!("找不到存活的代理 {to}；当前存活：{}", alive.join(", "))
            });
        }
        let clipped = crate::log::truncate_chars(content, MAX_MAIL_CHARS);
        let n = clipped.chars().count();
        let inbox = g.inboxes.entry(to.to_string()).or_default();
        if inbox.len() >= MAX_INBOX {
            return Err(format!(
                "{to} 的收件箱已满（{MAX_INBOX} 条未读），等它读过再发"
            ));
        }
        inbox.push(Mail {
            from: from.to_string(),
            content: clipped,
        });
        Ok(n)
    }

    /// 取走 `task_id` 的全部未读消息（`(from, content)`），取完即清空。
    pub fn drain(&self, task_id: &str) -> Vec<(String, String)> {
        let mut g = self.lock();
        g.inboxes
            .remove(task_id)
            .map(|v| v.into_iter().map(|m| (m.from, m.content)).collect())
            .unwrap_or_default()
    }
}

/// 进程级唯一登记处。工具实现（`tools.rs`）与子代理执行（`main.rs`）共用它。
static BUS: OnceLock<PeerBus> = OnceLock::new();

pub fn bus() -> &'static PeerBus {
    BUS.get_or_init(PeerBus::new)
}

// ── 「我是谁」：线程本地的当前 peer id ─────────────────────────────
//
// 工具实现拿不到 `run_subagent` 的 `task_id`（`tools::run` 的签名里没有它），而
// `ListPeers` 要标出「你」、`SendMessage` 要知道 `from` 是谁。用线程本地最省事：
// 每个子代理跑在自己的线程上（并行批 = `thread::scope` 新线程；串行批 = 主线程，
// 但有 [`enter`] 的 guard 负责还原），所以「本线程正在跑哪个 peer」是准确的。

thread_local! {
    static CURRENT: RefCell<Option<String>> = RefCell::new(None);
}

/// 把本线程标记为「正在跑 `task_id`」，返回的 guard 在 drop 时还原上一层。
/// 可嵌套（子代理里再跑 fork 技能会再进一层）。
pub struct CurrentGuard(Option<String>);

impl Drop for CurrentGuard {
    fn drop(&mut self) {
        let prev = self.0.take();
        CURRENT.with(|c| *c.borrow_mut() = prev);
    }
}

pub fn enter(task_id: &str) -> CurrentGuard {
    let prev = CURRENT.with(|c| c.borrow_mut().replace(task_id.to_string()));
    CurrentGuard(prev)
}

/// 本线程当前所属的 peer id（主循环线程上是 `None` ⇒ 调用方视作「主代理」）。
pub fn current() -> Option<String> {
    CURRENT.with(|c| c.borrow().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn register_list_unregister_roundtrip() {
        let b = PeerBus::new();
        assert!(b.list().is_empty());
        b.register("task-1", "alpha");
        b.register("task-2", "beta");
        assert_eq!(
            b.list(),
            vec![
                ("task-1".to_string(), "alpha".to_string()),
                ("task-2".to_string(), "beta".to_string()),
            ],
            "按注册顺序返回；顺序是稳定的"
        );
        // 同 id 重复登记只更新描述，不新增一行
        b.register("task-1", "alpha v2");
        assert_eq!(b.list().len(), 2);
        assert_eq!(b.list()[0].1, "alpha v2");
        b.unregister("task-1");
        assert_eq!(b.list(), vec![("task-2".to_string(), "beta".to_string())]);
    }

    #[test]
    fn send_requires_a_live_peer_and_drains_once() {
        let b = PeerBus::new();
        // 没有存活 peer ⇒ 发不出去（主代理在被阻塞时就是这个形态）
        let e = b.send("main", "task-1", "hi").unwrap_err();
        assert!(e.contains("没有正在运行的代理"), "实际：{e}");

        b.register("task-1", "alpha");
        b.register("task-2", "beta");
        // 发给不存在的 id ⇒ 报错里带上当前存活名单（便于模型自我纠正）
        let e = b.send("task-1", "task-9", "hi").unwrap_err();
        assert!(e.contains("task-9") && e.contains("task-1") && e.contains("task-2"), "实际：{e}");

        b.send("task-1", "task-2", "  配置在 src/a.rs  ").unwrap();
        let got = b.drain("task-2");
        assert_eq!(got, vec![("task-1".to_string(), "配置在 src/a.rs".to_string())]);
        assert!(b.drain("task-2").is_empty(), "drain 取走即清空，不重复投递");
    }

    #[test]
    fn empty_content_rejected_and_content_is_capped() {
        let b = PeerBus::new();
        b.register("task-1", "alpha");
        let e = b.send("main", "task-1", "   \n  ").unwrap_err();
        assert!(e.contains("content"), "实际：{e}");

        let long = "字".repeat(MAX_MAIL_CHARS + 500);
        let n = b.send("main", "task-1", &long).unwrap();
        // `log::truncate_chars` 在截断处补一个 "…(truncated)" 标记 ⇒ 结果略多于 MAX_MAIL_CHARS，
        // 但一定**远小于**原文（2500 字）。这里钉住「被截过」与「返回值 = 实际存下的长度」。
        assert!(n > MAX_MAIL_CHARS && n < long.chars().count(), "实际截断到 {n} 字");
        let got = b.drain("task-1");
        assert_eq!(got[0].1.chars().count(), n, "send 报的长度就是收件箱里存的长度");
    }

    #[test]
    fn full_inbox_is_rejected_not_silently_dropped() {
        let b = PeerBus::new();
        b.register("task-1", "alpha");
        for i in 0..MAX_INBOX {
            b.send("main", "task-1", &format!("m{i}")).unwrap();
        }
        let e = b.send("main", "task-1", "overflow").unwrap_err();
        assert!(e.contains("收件箱已满"), "实际：{e}");
        assert_eq!(b.drain("task-1").len(), MAX_INBOX, "被拒的那条不能混进队列");
    }

    #[test]
    fn unregister_clears_inbox() {
        let b = PeerBus::new();
        b.register("task-1", "alpha");
        b.send("main", "task-1", "later").unwrap();
        b.unregister("task-1");
        assert!(b.drain("task-1").is_empty(), "peer 注销后收件箱一并清掉");
        // 注销后再发 ⇒ 报「没有正在运行的代理」
        let e = b.send("main", "task-1", "x").unwrap_err();
        assert!(e.contains("没有正在运行的代理"), "实际：{e}");
    }

    #[test]
    fn current_guard_nests_and_restores() {
        assert_eq!(current(), None, "测试线程初始无归属");
        {
            let _outer = enter("task-1");
            assert_eq!(current().as_deref(), Some("task-1"));
            {
                let _inner = enter("skill-9");
                assert_eq!(current().as_deref(), Some("skill-9"));
            }
            assert_eq!(current().as_deref(), Some("task-1"), "内层退出还原外层");
        }
        assert_eq!(current(), None, "最外层退出还原成「主代理」");
    }
}

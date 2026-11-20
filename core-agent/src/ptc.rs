// PTC（Programmatic Tool Calling / Code Mode）最小可用实现 —— §待办 3-28.15。
//
// 形态：新增工具 `RunScript`（一次 `tool_use` 里跑完多次只读工具调用）。
// 模型写一段 JavaScript，代码里通过 `lunac.read/glob/grep` 调工具，用 `console.log`
// 把最终结果打出来 —— 中间结果不必逐条进模型上下文，也就不必每条一次往返。
//
// **安全口径**（见 `docs/ptc-research.md` §3）：
//   1. 绑定表**只有只读工具**（`Read` / `Glob` / `Grep`），实现上还再做一次名字白名单
//      —— 脚本可以绕过 JS 门面直接调 `__lunacCall`，所以 Rust 侧必须自己拒未绑定的名字；
//   2. 没有 `require` / `fs` / 网络：QuickJS 只拿到我们塞进去的三件门面，**默认断网**；
//   3. 硬预算：50 次工具调用 / 60 秒墙钟 / 256 KiB 中间结果 / 64 K 字符输出 /
//      64 MiB 堆 + 1 MiB 栈；超限立即终止并如实报；
//   4. 外层 `RunScript` **每次都要审批**（`tools::needs_approval`）—— 脚本内容用户看得到。
//
// **隔离边界如实记**（`forbidden.md` §禁用 1）：这是**进程内嵌** QuickJS + 策略级绑定白名单，
// **不是** OS 级沙箱。它能挡住「脚本乱调工具 / 死循环 / 吃内存」，挡不住 QuickJS 自身漏洞
// （Check Point 2026 在同类运行时上报过 RCE）—— 所以它默认关闭，只作为可选模式。

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

/// 一次脚本最多调多少次工具。
pub const MAX_STEPS: usize = 50;
/// 墙钟上限（不含内层工具自身耗时之外的等待）。
pub const WALL_CLOCK: Duration = Duration::from_secs(60);
/// 所有工具返回给脚本的正文累计上限。
pub const MAX_MID_BYTES: usize = 256 * 1024;
/// 回给模型的 `console.log` 输出上限（字符）。
pub const MAX_OUTPUT_CHARS: usize = 64 * 1024;
/// 脚本正文上限（字符）。
pub const MAX_CODE_CHARS: usize = 20_000;
/// QuickJS 堆 / 栈上限。
const MEMORY_LIMIT: usize = 64 * 1024 * 1024;
const STACK_LIMIT: usize = 1024 * 1024;

/// 绑定表：**只读**工具。与 `tools::parallel_safe` 的口径一致，但这里只挑三件
/// 确知「不写盘、不发请求、不依赖调用顺序」的（`WebSearch` / `WebFetch` 要审批，不进）。
pub const BOUND_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

/// 宿主开关（`LUNAC_PTC=on`）；关着时 `RunScript` 根本不进工具表。
pub fn enabled() -> bool {
    matches!(
        std::env::var("LUNAC_PTC").ok().as_deref().map(str::trim),
        Some("on") | Some("1") | Some("true")
    )
}

struct TraceStep {
    tool: String,
    ok: bool,
    ms: u128,
    chars: usize,
}

struct State {
    ctx: crate::tools::Ctx,
    started: Instant,
    steps: usize,
    mid_bytes: usize,
    trace: Vec<TraceStep>,
    output: String,
    output_truncated: bool,
    stop_reason: Option<String>,
}

/// `RunScript` 的执行入口（`tools::run` 的一条分支）。
pub fn run(ctx: &crate::tools::Ctx, input: &Value) -> Result<String, String> {
    let code = input
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if code.is_empty() {
        return Err("缺少 code 参数：要跑的脚本不能为空".to_string());
    }
    let code_chars = code.chars().count();
    if code_chars > MAX_CODE_CHARS {
        return Err(format!(
            "脚本过长（{code_chars} 字符 > 上限 {MAX_CODE_CHARS}）：请把逻辑简化后再提交"
        ));
    }

    let state = Rc::new(RefCell::new(State {
        ctx: ctx.clone(),
        started: Instant::now(),
        steps: 0,
        mid_bytes: 0,
        trace: Vec::new(),
        output: String::new(),
        output_truncated: false,
        stop_reason: None,
    }));

    let runtime = rquickjs::Runtime::new().map_err(|e| format!("JS 运行时初始化失败：{e}"))?;
    runtime.set_memory_limit(MEMORY_LIMIT);
    runtime.set_max_stack_size(STACK_LIMIT);
    // 墙钟闸：QuickJS 在跑字节码时会周期性问这个闭包，返回 true = 立刻中断。
    // 注意它**管不到**正在执行的 Rust 回调（`Read` / `Glob` / `Grep` 自身很快）。
    let deadline = Instant::now() + WALL_CLOCK;
    runtime.set_interrupt_handler(Some(Box::new(move || Instant::now() >= deadline)));

    let js = rquickjs::Context::full(&runtime)
        .map_err(|e| format!("JS 上下文创建失败：{e}"))?;

    let mut thrown: Option<String> = None;
    let outcome = js.with(|jsctx| -> Result<(), String> {
        let globals = jsctx.globals();

        // ① 工具门面：脚本唯一能碰到宿主的地方。
        let for_call = state.clone();
        let call = rquickjs::Function::new(
            jsctx.clone(),
            move |tool: String, args: String| -> String {
                tool_call(&for_call, &tool, &args)
            },
        )
        .map_err(|e| format!("绑定工具门面失败：{e}"))?;
        globals
            .set("__lunacCall", call)
            .map_err(|e| format!("注册工具门面失败：{e}"))?;

        // ② console.log：只收文本，进输出缓冲（超限置截断标记，不抛）。
        let for_log = state.clone();
        let log = rquickjs::Function::new(jsctx.clone(), move |line: String| {
            let mut st = for_log.borrow_mut();
            if st.output.chars().count() >= MAX_OUTPUT_CHARS {
                st.output_truncated = true;
                return;
            }
            st.output.push_str(&line);
            st.output.push('\n');
        })
        .map_err(|e| format!("绑定 console.log 失败：{e}"))?;
        globals
            .set("__lunacLog", log)
            .map_err(|e| format!("注册 console.log 失败：{e}"))?;

        // ③ 门面 + 预置全局（`lunac` / `console`）。包在 IIFE 里，不往全局泄漏辅助函数。
        let prelude = "\
(function () {\n\
  const call = (tool, args) => {\n\
    const r = JSON.parse(__lunacCall(tool, JSON.stringify(args || {})));\n\
    if (!r.ok) throw new Error(r.error);\n\
    return r.text;\n\
  };\n\
  globalThis.lunac = {\n\
    read: (path, opts) => call('Read', Object.assign({ file_path: path }, opts || {})),\n\
    glob: (pattern, opts) => call('Glob', Object.assign({ pattern: pattern }, opts || {})),\n\
    grep: (pattern, opts) => call('Grep', Object.assign({ pattern: pattern }, opts || {})),\n\
  };\n\
  globalThis.console = {\n\
    log: (...a) => __lunacLog(a.map(x => typeof x === 'string' ? x : JSON.stringify(x)).join(' ')),\n\
  };\n\
})();\n";
        eval_unit(&jsctx, prelude)?;
        match jsctx.eval::<(), _>(code) {
            Ok(()) => Ok(()),
            Err(error) => {
                thrown = Some(describe_exception(&jsctx, &error));
                Err(thrown.clone().unwrap_or_default())
            }
        }
    });
    // 运行时用完即弃：出任何错都不影响主进程（上下文随 `js` 释放）。
    drop(js);

    let snapshot = state.borrow();
    let report = format_report(&snapshot);
    // 预算 / 白名单触顶时，脚本**即使把错误吞掉**也必须让这次调用失败 ——
    // 否则模型会看到「成功」，然后按一份被截断的结果继续推理。
    let stop = snapshot.stop_reason.clone();
    drop(snapshot);
    match outcome {
        Ok(()) if stop.is_none() => Ok(report),
        _ => {
            // 超步数 / 超时 / 未绑定工具这类「我们自己拒绝的」带原因；语法 / 运行时错误带 JS 消息。
            let reason = stop
                .or(thrown)
                .unwrap_or_else(|| "脚本执行失败".to_string());
            Err(format!("RunScript 失败：{reason}\n\n{report}"))
        }
    }
}

/// 一次工具调用（脚本 → 宿主）。**名字白名单在这里再判一次**，不能只靠 JS 门面。
fn tool_call(state: &Rc<RefCell<State>>, tool: &str, args: &str) -> String {
    let mut st = state.borrow_mut();
    if let Some(reason) = &st.stop_reason {
        return error_json(reason);
    }
    if !BOUND_TOOLS.contains(&tool) {
        let reason = format!(
            "RunScript 只绑定了只读工具（{}）；「{tool}」不可用",
            BOUND_TOOLS.join(" / ")
        );
        st.stop_reason = Some(reason.clone());
        return error_json(&reason);
    }
    if st.steps >= MAX_STEPS {
        let reason = format!("已达到 {MAX_STEPS} 次工具调用上限：请把脚本拆小或减少循环");
        st.stop_reason = Some(reason.clone());
        return error_json(&reason);
    }
    if st.started.elapsed() >= WALL_CLOCK {
        let reason = format!("已超过 {} 秒墙钟上限", WALL_CLOCK.as_secs());
        st.stop_reason = Some(reason.clone());
        return error_json(&reason);
    }
    st.steps += 1;

    let parsed: Value = serde_json::from_str(args).unwrap_or_else(|_| json!({}));
    let began = Instant::now();
    let result = crate::tools::run(&st.ctx, tool, &parsed);
    let ms = began.elapsed().as_millis();
    match result {
        Ok(text) => {
            st.mid_bytes += text.len();
            let chars = text.chars().count();
            st.trace.push(TraceStep {
                tool: tool.to_string(),
                ok: true,
                ms,
                chars,
            });
            if st.mid_bytes > MAX_MID_BYTES {
                let reason = format!(
                    "工具返回累计超过 {} KiB：请在脚本里只取需要的片段（offset/limit、更窄的 pattern）",
                    MAX_MID_BYTES / 1024
                );
                st.stop_reason = Some(reason.clone());
                return error_json(&reason);
            }
            json!({ "ok": true, "text": text }).to_string()
        }
        Err(message) => {
            st.trace.push(TraceStep {
                tool: tool.to_string(),
                ok: false,
                ms,
                chars: 0,
            });
            error_json(&message)
        }
    }
}

fn error_json(reason: &str) -> String {
    json!({ "ok": false, "error": reason }).to_string()
}

fn eval_unit(ctx: &rquickjs::Ctx<'_>, code: &str) -> Result<(), String> {
    ctx.eval::<(), _>(code)
        .map_err(|error| describe_exception(ctx, &error))
}

/// 把 QuickJS 的异常翻成一句人话（`Error` 取 `message`，其它值取字符串形式）。
fn describe_exception(ctx: &rquickjs::Ctx<'_>, error: &rquickjs::Error) -> String {
    if !matches!(error, rquickjs::Error::Exception) {
        return format!("{error}");
    }
    let value = ctx.catch();
    if let Some(text) = value.as_string() {
        return text.to_string().unwrap_or_default();
    }
    if let Some(object) = value.as_object() {
        if let Ok(message) = object.get::<&str, String>("message") {
            return message;
        }
    }
    format!("{error}")
}

/// 回给模型的报告：摘要 + `console.log` 输出 + 逐步轨迹（`tool / ok / ms / chars`）。
fn format_report(state: &State) -> String {
    let elapsed = state.started.elapsed().as_millis();
    let mut out = format!(
        "[RunScript] {} 次工具调用 · {}ms · 中间结果 {} 字节\n",
        state.steps,
        elapsed,
        state.mid_bytes
    );
    if state.output.is_empty() {
        out.push_str("--- output ---\n(no console.log output)\n");
    } else {
        out.push_str("--- output ---\n");
        out.push_str(&state.output);
        if state.output_truncated {
            out.push_str("…(output truncated at 64K chars)\n");
        }
    }
    if !state.trace.is_empty() {
        out.push_str("--- trace ---\n");
        for (index, step) in state.trace.iter().enumerate() {
            out.push_str(&format!(
                "{}. {} {} {}ms {} chars\n",
                index + 1,
                step.tool,
                if step.ok { "ok" } else { "error" },
                step.ms,
                step.chars
            ));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn ctx_with(root: &std::path::Path) -> crate::tools::Ctx {
        use std::sync::atomic::AtomicBool;
        use std::sync::{Arc, Mutex};
        crate::tools::Ctx {
            cwd: root.to_path_buf(),
            add_dirs: Vec::new(),
            read_only: false,
            locked: false,
            deny_read_dirs: Vec::new(),
            worktree: Arc::new(Mutex::new(None)),
            plan_phase: Arc::new(AtomicBool::new(false)),
            tool_output: None,
            shell_control: None,
            memory_scope: crate::tools::MemoryScope::Global,
            skill_scope: None,
        }
    }

    #[test]
    fn budgets_and_bindings_are_declared_as_documented() {
        // 绑定表只读：写类 / 命令类 / 网络类一个都不在里面。
        for name in BOUND_TOOLS {
            assert!(!crate::tools::needs_approval(name), "{name} 必须免审批");
        }
        assert_eq!(MAX_STEPS, 50);
        assert_eq!(WALL_CLOCK.as_secs(), 60);
    }

    #[test]
    fn a_script_can_chain_read_only_tools_and_return_output() {
        let dir = std::env::temp_dir().join(format!("lunac-ptc-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "alpha\n").unwrap();
        std::fs::write(dir.join("b.txt"), "beta\n").unwrap();
        let ctx = ctx_with(&dir);

        let script = "const files = ['a.txt', 'b.txt'];\n\
                      const out = files.map((f) => lunac.read(f));\n\
                      console.log(out.join('|'));\n";
        let report = run(&ctx, &json!({ "code": script })).expect("script runs");
        assert!(report.contains("alpha") && report.contains("beta"));
        assert!(report.contains("2 次工具调用"), "{report}");
        assert!(report.contains("1. Read ok"), "{report}");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unbound_tools_are_rejected_even_when_the_script_calls_the_raw_hook() {
        let dir = std::env::temp_dir().join(format!("lunac-ptc-guard-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let ctx = ctx_with(&dir);

        let script = "__lunacCall('Write', JSON.stringify({ file_path: 'x.txt', content: 'no' }));";
        let error = run(&ctx, &json!({ "code": script })).expect_err("must be rejected");
        assert!(error.contains("只绑定了只读工具"), "{error}");
        assert!(!dir.join("x.txt").exists(), "写类工具绝不能被执行");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn empty_or_oversized_scripts_are_rejected() {
        let dir = std::env::temp_dir().join(format!("lunac-ptc-empty-{}", std::process::id()));
        let ctx = ctx_with(&dir);
        assert!(run(&ctx, &json!({ "code": "   " })).is_err());
        let huge = "x".repeat(MAX_CODE_CHARS + 1);
        assert!(run(&ctx, &json!({ "code": huge })).is_err());
    }
}

//! `WebBrowser` 工具（2026-10-23，A13 最后一项）：用**本机已装的 Chromium 内核浏览器**做
//! 「一次无头渲染」——把 JS 跑完之后的 DOM 取回来，供模型读 SPA / 需要脚本才能出内容的页面。
//!
//! **与 `WebFetch` 的分工**（两者都保留）：
//!   · `WebFetch` = 静态 HTTP GET（reqwest，不跑 JS）——快、省、够读文档 / API / 静态页；
//!   · `WebBrowser` = 真浏览器渲染（`--headless=new --dump-dom`）——慢、要起进程，但**跑 JS**，
//!     适合「页面打开时才有内容」的站点。
//!
//! **零新依赖、零新下载**：直接调用本机 Edge / Chrome（Windows 一定自带 Edge），
//! 不引入 Playwright / Puppeteer，也不下载浏览器内核。
//!
//! ⚠️ **诚实边界**（与 `forbidden.md` §禁用 1 同口径）：
//!   · **不是沙箱**：浏览器以当前用户身份运行，能读该用户的 cookie / 本地存储 ——
//!     故每次都用**独立临时 profile**（`--user-data-dir`，用完即删），不碰用户日常配置；
//!   · **只有「打开 + 等待 + 取 DOM」**，没有 `click` / `type` / 截图 —— 那需要 CDP 长连接
//!     （常驻会话），本轮不做（archive backlog 里 `WebBrowser` 的原描述就是「需要常驻上下文」）；
//!   · **每次调用起一次浏览器**（不缓存进程），耗时约 1–5 s 起 + `wait_ms`；
//!   · 外部网络出口 ⇒ **要审批**、只读档也要问（与 `WebFetch` 同级）。
//!
//! 失败一律返回**可读错误**（没装浏览器 / 启动失败 / 超时 / 页面空白），绝不是静默空串。

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// 等 JS / 网络的虚拟时间预算：默认与上限（毫秒）。
const DEFAULT_WAIT_MS: u64 = 5_000;
const MAX_WAIT_MS: u64 = 20_000;
/// 浏览器进程的总时限 = `wait_ms` + 这段（冷启动 / 退出清理）。
const STARTUP_GRACE_MS: u64 = 20_000;
/// 取回的 DOM 上限（页面可能几十 MB；超出部分直接丢弃，只留前 N 字节）。
const MAX_DOM_BYTES: usize = 8 * 1024 * 1024;
/// 纯文本 / 原始 DOM 的输出上限（字符）。
const MAX_TEXT_CHARS: usize = 40_000;
const MAX_HTML_CHARS: usize = 60_000;
/// 失败时带回的浏览器 stderr 尾巴。
const MAX_STDERR_CHARS: usize = 800;

/// 找可用的 Chromium 内核浏览器：`LUNAC_BROWSER` 显式指定 > PATH > 常见安装路径。
///
/// 顺序上先 Edge（Windows 必装），再 Chrome；两者命令行（headless / dump-dom）同源。
fn find_browser() -> Option<PathBuf> {
    if let Some(custom) = std::env::var_os("LUNAC_BROWSER") {
        let p = PathBuf::from(custom);
        if p.is_file() {
            return Some(p);
        }
    }
    find_on_path("msedge")
        .or_else(|| find_on_path("chrome"))
        .or_else(|| find_on_path("chromium"))
        .or_else(|| {
            browser_candidates()
                .into_iter()
                .map(PathBuf::from)
                .find(|p| p.is_file())
        })
}

/// 各平台的常见安装路径（纯列表，便于单测钉住「Edge 与 Chrome 都覆盖」）。
fn browser_candidates() -> Vec<&'static str> {
    #[cfg(windows)]
    {
        vec![
            r"C:\Program Files (x86)\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
            r"C:\Program Files\Google\Chrome\Application\chrome.exe",
            r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        ]
    }
    #[cfg(target_os = "macos")]
    {
        vec![
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        ]
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        vec![
            "/usr/bin/google-chrome",
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/microsoft-edge",
        ]
    }
}

/// 在 PATH 上找可执行文件（Windows 额外试 `.exe`）—— 与 `lsp.rs` 的同名逻辑各自独立即可
/// （两者分属两个工具，没有共享状态；合并反而会让「删掉一个工具」牵连另一个）。
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path_var = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path_var) {
        let direct = dir.join(name);
        if direct.is_file() {
            return Some(direct);
        }
        #[cfg(windows)]
        {
            let exe = dir.join(format!("{name}.exe"));
            if exe.is_file() {
                return Some(exe);
            }
        }
    }
    None
}

/// 命令行参数（纯函数，便于单测）：**只允许**打开 + 等 JS + 取 DOM。
fn browser_args(profile: &Path, wait_ms: u64, url: &str) -> Vec<String> {
    let mut args = vec![
        "--headless=new".to_string(),
        // 取「跑完 JS 之后」的 DOM 到 stdout —— 这是本工具与 `WebFetch` 的全部差别。
        "--dump-dom".to_string(),
        "--disable-gpu".to_string(),
        "--hide-scrollbars".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-extensions".to_string(),
        "--mute-audio".to_string(),
        format!("--user-data-dir={}", profile.display()),
    ];
    if wait_ms > 0 {
        args.push(format!("--virtual-time-budget={wait_ms}"));
    }
    // URL 必须**最后**一个：Chromium 把第一个非开关参数当目标页。
    args.push(url.to_string());
    args
}

/// 工具入口（由 `tools::run` 分派）。
pub fn run(input: &Value) -> Result<String, String> {
    // URL 校验复用 `WebFetch` 那一套（http→https 升级、长度封顶），不新造第二个判据。
    let url = crate::tools::normalize_url(
        input
            .get("url")
            .and_then(Value::as_str)
            .ok_or_else(|| "missing required parameter \"url\"".to_string())?,
    )?;
    let wait_ms = input
        .get("wait_ms")
        .and_then(Value::as_u64)
        .unwrap_or(DEFAULT_WAIT_MS)
        .min(MAX_WAIT_MS);
    let format = input
        .get("format")
        .and_then(Value::as_str)
        .unwrap_or("text")
        .to_ascii_lowercase();

    let browser = find_browser().ok_or_else(|| {
        "WebBrowser unavailable: no Edge / Chrome found. Install Microsoft Edge or Google Chrome, \
         or point LUNAC_BROWSER at the browser executable."
            .to_string()
    })?;

    let profile = std::env::temp_dir().join(format!(
        "lunac-webbrowser-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&profile)
        .map_err(|e| format!("WebBrowser failed: cannot create profile {}: {e}", profile.display()))?;

    let result = render(&browser, &profile, &url, wait_ms, &format);
    // 无论成败都清掉临时 profile（里面可能有 cookie / 缓存）。
    let _ = std::fs::remove_dir_all(&profile);
    result
}

fn render(
    browser: &Path,
    profile: &Path,
    url: &str,
    wait_ms: u64,
    format: &str,
) -> Result<String, String> {
    let mut cmd = Command::new(browser);
    cmd.args(browser_args(profile, wait_ms, url))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW —— GUI 宿主下不闪黑框
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("WebBrowser failed: cannot start {}: {e}", browser.display()))?;

    // 必须**边跑边读**：DOM 动辄几百 KB，只等进程结束再读会顶满管道缓冲区、把双方卡死。
    let out_pipe = child.stdout.take();
    let err_pipe = child.stderr.take();
    let out_reader = out_pipe.map(|p| std::thread::spawn(move || drain(p, MAX_DOM_BYTES)));
    let err_reader = err_pipe.map(|p| std::thread::spawn(move || drain(p, 16 * 1024)));

    let deadline = Instant::now() + Duration::from_millis(wait_ms + STARTUP_GRACE_MS);
    let exited = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    break None;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("WebBrowser failed: wait on browser: {e}"));
            }
        }
    };

    let dom = join_reader(out_reader);
    let stderr = join_reader(err_reader);
    let stderr_tail = crate::log::truncate_chars(stderr.trim(), MAX_STDERR_CHARS);

    let status = match exited {
        Some(s) => s,
        None => {
            return Err(format!(
                "WebBrowser timed out after {} ms (waited {wait_ms} ms for the page){}",
                wait_ms + STARTUP_GRACE_MS,
                if stderr_tail.is_empty() {
                    String::new()
                } else {
                    format!("; stderr: {stderr_tail}")
                }
            ))
        }
    };
    if !status.success() && dom.trim().is_empty() {
        return Err(format!(
            "WebBrowser failed: browser exited with {} and produced no DOM{}",
            status,
            if stderr_tail.is_empty() {
                String::new()
            } else {
                format!("; stderr: {stderr_tail}")
            }
        ));
    }
    if dom.trim().is_empty() {
        return Err(format!(
            "WebBrowser {url} → empty DOM (the page may require interaction or blocked the browser)"
        ));
    }

    let title = extract_title(&dom);
    let truncated;
    let body = if format == "html" {
        let full = dom.trim();
        truncated = full.chars().count() > MAX_HTML_CHARS;
        crate::log::truncate_chars(full, MAX_HTML_CHARS)
    } else {
        let full = crate::tools::html_to_text(&dom);
        truncated = full.chars().count() > MAX_TEXT_CHARS;
        crate::log::truncate_chars(&full, MAX_TEXT_CHARS)
    };
    if body.trim().is_empty() {
        return Err(format!(
            "WebBrowser {url} → page rendered but contains no readable text \
             (try format=\"html\" to inspect the DOM)"
        ));
    }

    let mut out = format!(
        "URL: {url}\nTitle: {}\nRendered: headless {} (waited {wait_ms} ms)\n\n{body}",
        if title.is_empty() { "(none)" } else { &title },
        browser
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_else(|| "browser".to_string())
    );
    if truncated {
        // 只在**真的截断**时才说 —— 否则每页都挂一句「已截断」，模型会误判内容不全。
        out.push_str(&format!(
            "\n\n… ({} truncated at {} chars)",
            if format == "html" { "DOM" } else { "text" },
            if format == "html" {
                MAX_HTML_CHARS
            } else {
                MAX_TEXT_CHARS
            }
        ));
    }
    Ok(out)
}

/// 读管道：最多保留 `cap` 字节，**其余继续读掉**（不读就会堵住子进程）。
fn drain(mut reader: impl Read, cap: usize) -> Vec<u8> {
    let mut kept: Vec<u8> = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if kept.len() < cap {
                    let room = cap - kept.len();
                    kept.extend_from_slice(&buf[..n.min(room)]);
                }
            }
            Err(_) => break,
        }
    }
    kept
}

fn join_reader(handle: Option<std::thread::JoinHandle<Vec<u8>>>) -> String {
    match handle.and_then(|h| h.join().ok()) {
        Some(bytes) => String::from_utf8_lossy(&bytes).to_string(),
        None => String::new(),
    }
}

/// 从 DOM 里取 `<title>`（只在 head 区域找；取不到回空串）。
fn extract_title(dom: &str) -> String {
    let head: String = dom.chars().take(200_000).collect();
    let Ok(re) = regex::Regex::new(r"(?is)<title[^>]*>(.*?)</title\s*>") else {
        return String::new();
    };
    let Some(caps) = re.captures(&head) else {
        return String::new();
    };
    crate::tools::html_to_text(caps.get(1).map(|m| m.as_str()).unwrap_or(""))
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    /// 参数必须**只读**（headless + dump-dom）且 URL 在最后；`wait_ms=0` 时不带虚拟时间预算。
    #[test]
    fn args_are_headless_and_url_comes_last() {
        let args = browser_args(Path::new(r"D:\tmp\profile"), 5_000, "https://example.com/a?b=1");
        assert!(args.iter().any(|a| a == "--headless=new"));
        assert!(args.iter().any(|a| a == "--dump-dom"), "{args:?}");
        assert!(args
            .iter()
            .any(|a| a == "--user-data-dir=D:\\tmp\\profile"));
        assert!(args.iter().any(|a| a == "--virtual-time-budget=5000"));
        assert_eq!(args.last().unwrap(), "https://example.com/a?b=1");
        // 不带任何交互类开关（本轮刻意只做「打开 + 取 DOM」）。
        assert!(!args.iter().any(|a| a.contains("remote-debugging")));

        let no_wait = browser_args(Path::new("p"), 0, "https://example.com");
        assert!(!no_wait.iter().any(|a| a.starts_with("--virtual-time-budget")));
    }

    /// `wait_ms` 必须有上限（页面要等 JS 时也别让一次调用挂几分钟）。
    #[test]
    fn wait_is_clamped_by_the_caller() {
        let big = 999_999u64.min(MAX_WAIT_MS);
        assert_eq!(big, 20_000);
        assert!(MAX_WAIT_MS >= DEFAULT_WAIT_MS);
    }

    /// 常见安装路径必须同时覆盖 Edge 与 Chrome（否则「本机只有其中一个」时直接不可用）。
    #[test]
    fn candidates_cover_edge_and_chrome() {
        let list = browser_candidates();
        assert!(!list.is_empty());
        let joined = list.join("|").to_ascii_lowercase();
        assert!(joined.contains("msedge"), "{joined}");
        assert!(joined.contains("chrome"), "{joined}");
    }

    /// 标题提取：实体解码 + 大小写标签都能认；没有标题回空串。
    #[test]
    fn title_is_extracted_and_entities_decoded() {
        let dom = "<HTML><HEAD><TITLE>Tom &amp; Jerry</TITLE></HEAD><BODY>x</BODY></HTML>";
        assert_eq!(extract_title(dom), "Tom & Jerry");
        assert_eq!(extract_title("<html><body>no title</body></html>"), "");
    }

    /// 读管道：超过上限只保留前 `cap` 字节，但**必须继续读空管道**（否则子进程会堵）。
    #[test]
    fn drain_keeps_at_most_cap_bytes_and_consumes_the_rest() {
        let payload = vec![b'x'; 100_000];
        let kept = drain(Cursor::new(payload), 1_000);
        assert_eq!(kept.len(), 1_000);
    }

    /// **真机冒烟**（默认忽略）：直接驱动本机 Edge/Chrome 渲染一个 `data:` 页面（不联网），
    /// 钉住「参数真的能起、DOM 真的能读回、标题/正文真的能转」。
    /// 跑法：`cargo test --bins webbrowser_real_browser_smoke -- --ignored --nocapture`
    #[test]
    #[ignore = "需要本机 Edge/Chrome；手动跑（见注释里的命令）"]
    fn webbrowser_real_browser_smoke() {
        let Some(browser) = find_browser() else {
            eprintln!("skip: 本机没有 Edge / Chrome");
            return;
        };
        let profile = std::env::temp_dir().join(format!("lunac-wb-smoke-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&profile);
        std::fs::create_dir_all(&profile).expect("profile");
        let out = render(
            &browser,
            &profile,
            "data:text/html,<html><head><title>T1</title></head><body><h1>Hello</h1></body></html>",
            2_000,
            "text",
        );
        let _ = std::fs::remove_dir_all(&profile);
        let text = out.expect("应能渲染 data: URL");
        eprintln!("WebBrowser smoke => {text}");
        assert!(text.contains("Hello"), "{text}");
        assert!(text.contains("Title: T1"), "{text}");
        // 不该把残留的临时 profile 留在那儿（render 本身不清，run 负责清 —— 这里只确认路径已删）。
        assert!(!profile.exists());
    }
}

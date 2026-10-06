// core-agent/src/image.rs
//! `ImageGen`（A13，2026-10-03）：agent 的**出图**工具。
//!
//! **它与 `WebFetch` 是同类**：都会把数据发往外部、都把结果落成本机文件，所以
//! `needs_approval` + 只读档走 `tools::write_blocked`（见 tools.rs 的 `run`）。
//!
//! ## 为什么单开一个模块、而不是塞进 tools.rs
//!
//! 出图**不活在 OpenAI 兼容 chat 端点上** —— 文本侧走 `…/compatible-mode`，出图走
//! DashScope 的多点编辑端点（`multimodal-generation/generation`）。两者的请求体形状
//! 完全不同（这里没有 `messages` 的角色语义，content 是「若干图 + 一段文字」的混合数组），
//! 把它塞进 tools.rs 只会让那个文件里多出一段与工具分发无关的协议代码。
//!
//! ## 三个实测过的硬约束（2026-10-02 用真 key 逐个撞出来的，别再试）
//!
//! ① **单次最多 3 张输入图**；② **输出单边 512–2048**（超了报
//! `Width and height must be between 512 and 2048 pixels`）；③ `prompt_extend`
//! **会自动改写提示词**、把「精致高质量」这类词加回来 —— 与「去 AI 味」的诉求正好相反，
//! 所以默认 **关**（`enhance` 参数显式打开才开）。
//!
//! ## 落盘位置
//!
//! `<exe 根>\temp\images\`（与 `tools::output_dir()` 同级，但**不共用**：那个目录有
//! 7 天清理策略，生成的图不该跟着被清）。main.rs 会把它推进 `Ctx.add_dirs`，
//! 否则工作区锁会让模型读不到自己刚生成的图。

use std::env;
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde_json::{json, Value};

/// DashScope 多点编辑端点（实测可用，见模块头）。`LUNAC_IMAGE_URL` 可覆盖。
const DEFAULT_ENDPOINT: &str =
    "https://dashscope.aliyuncs.com/api/v1/services/aigc/multimodal-generation/generation";
/// 默认尺寸。**必须带 `*`**（DashScope 用 `W*H`，不是 `x`）。
const DEFAULT_SIZE: &str = "1024*1024";
/// 单次输入图上限（DashScope 的硬限制，见模块头 ①）。
const MAX_INPUT_IMAGES: usize = 3;
/// 单张输入图字节上限 —— 超过就不发（base64 后还要再涨 1/3，请求体会大到端点拒绝）。
const MAX_INPUT_BYTES: u64 = 10 * 1024 * 1024;
/// 出图超时：实测一张要几十秒，给足余量。
const GEN_TIMEOUT_SECS: u64 = 300;
/// `n` 上限。**按张计费**，所以设一个不夸张的盖子（防止模型一次要 20 张）。
const MAX_N: u64 = 4;
/// 边长的合法区间（见模块头 ②）。
const MIN_SIDE: u32 = 512;
const MAX_SIDE: u32 = 2048;

/// 出图功能是否已配置（宿主注入 `LUNAC_IMAGE_MODEL` 才算配了）。
/// main.rs 用它决定**要不要把 `ImageGen` 注册进工具池** —— 没配就注册，等于在固定
/// 前缀里放一件必然失败的工具（与 `Remember` 的条件注册同一条纪律）。
pub fn enabled() -> bool {
    env::var("LUNAC_IMAGE_MODEL")
        .map(|v| !v.trim().is_empty())
        .unwrap_or(false)
}

fn endpoint() -> String {
    env::var("LUNAC_IMAGE_URL")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_ENDPOINT.to_string())
}

fn api_key() -> Result<String, String> {
    // 优先专用变量；宿主没注入时回落到 agent 自己的 token（同一把 DashScope key）。
    env::var("LUNAC_IMAGE_KEY")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| env::var("LUNAC_AGENT_TOKEN").ok().filter(|v| !v.trim().is_empty()))
        .ok_or_else(|| "no image API key configured (set it in Lunac settings)".to_string())
}

/// 出图落盘目录：`<exe 根>\temp\images`。**不新增环境变量** —— 与 `tools::output_dir()`
/// 同法，从 `log::log_dir()` 的父目录派生（它已经处理了「宿主注入 LUNAC_LOG_DIR，
/// 否则回退 exe 目录」两种情况）。
pub fn image_dir() -> PathBuf {
    crate::log::log_dir()
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
        .join("images")
}

/// 启动时调用：建目录并返回路径（main.rs 要把它塞进 `Ctx.add_dirs`）。
pub fn prepare_image_dir() -> PathBuf {
    let dir = image_dir();
    if let Err(e) = fs::create_dir_all(&dir) {
        crate::log::warn(format!("images: 建目录失败 {}: {e}", dir.display()));
    }
    dir
}

/// `ImageGen` 的 schema。**不进 `tools::defs()`** —— 由 main.rs 条件注册（见 [`enabled`]）。
pub fn tool_def() -> Value {
    json!({
        "name": "ImageGen",
        "description": "Generate or edit images with an image model (Qwen-Image). Text-to-image \
            when `images` is omitted; instruction-based editing when `images` holds 1-3 local \
            file paths (the model edits those pictures rather than describing them). Returns the \
            absolute path(s) of the saved PNG(s) — view them with Read if needed. At most 3 input \
            images per call and each output side must be 512-2048. Generation takes tens of \
            seconds and is BILLED PER IMAGE, so keep `n` small.",
        "input_schema": {
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "What to draw, or — when `images` is given — the edit instruction (e.g. \"redraw only the face\"). Write it in the language the model documents; English or Chinese both work."
                },
                "images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional local file paths (1-3) of input images for editing. Absolute paths work best."
                },
                "size": {
                    "type": "string",
                    "description": "Output size as WIDTH*HEIGHT (e.g. \"1024*1536\"). Each side must be 512-2048. Default 1024*1024."
                },
                "n": {
                    "type": "integer",
                    "description": "How many images to generate, 1-4 (default 1). Each one is billed."
                },
                "negative_prompt": {
                    "type": "string",
                    "description": "What to avoid. Defaults to a short anti-artifact list when omitted."
                },
                "enhance": {
                    "type": "boolean",
                    "description": "Let the service rewrite your prompt (prompt_extend). Off by default — it tends to add generic \"high quality\" wording that makes the result look more AI-generated."
                }
            },
            "required": ["prompt"]
        }
    })
}

/// 一段保守的默认负向词（与 qwen_edit.py 实测那版同源）：主要防出图的常见畸变。
const DEFAULT_NEG: &str =
    "lowres, blurry, watermark, text, signature, extra limbs, bad anatomy, extra fingers";

/// 一图一段的 mime 判据（按 magic bytes，不看扩展名）。
fn media_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

/// 扩展名（落盘用）。判不出来按 `.png`（DashScope 出图就是 PNG）。
fn ext_for(bytes: &[u8]) -> &'static str {
    match media_type(bytes) {
        Some("image/jpeg") => "jpg",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        _ => "png",
    }
}

/// 把一张本地图读成 `data:` URI。**不缩放**（agent 里没有 image 解码依赖）⇒
/// 用字节上限兜住，超限直接报错而不是发一个会被端点拒绝的巨体。
fn data_uri(path: &str) -> Result<String, String> {
    let p = PathBuf::from(path);
    let meta = fs::metadata(&p).map_err(|e| format!("cannot read input image {path}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("input image is not a file: {path}"));
    }
    if meta.len() > MAX_INPUT_BYTES {
        return Err(format!(
            "input image {path} is {} MB, over the {} MB limit",
            meta.len() / 1_048_576,
            MAX_INPUT_BYTES / 1_048_576
        ));
    }
    let bytes = fs::read(&p).map_err(|e| format!("cannot read input image {path}: {e}"))?;
    let mime = media_type(&bytes)
        .ok_or_else(|| format!("{path} is not a PNG / JPEG / GIF / WebP (checked by magic bytes)"))?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
    Ok(format!("data:{mime};base64,{b64}"))
}

/// 校验 DashScope 的 `W*H` 尺寸串（见模块头 ②）。返回规范化后的串。
fn parse_size(raw: Option<&str>) -> Result<String, String> {
    let s = raw.map(str::trim).filter(|s| !s.is_empty()).unwrap_or(DEFAULT_SIZE);
    let bad = || {
        format!(
            "size must look like WIDTH*HEIGHT with each side between {MIN_SIDE} and {MAX_SIDE} \
             (e.g. \"1024*1536\"), got \"{s}\""
        )
    };
    let (w, h) = s.split_once(['*', 'x', 'X']).ok_or_else(bad)?;
    let w: u32 = w.trim().parse().map_err(|_| bad())?;
    let h: u32 = h.trim().parse().map_err(|_| bad())?;
    if !(MIN_SIDE..=MAX_SIDE).contains(&w) || !(MIN_SIDE..=MAX_SIDE).contains(&h) {
        return Err(bad());
    }
    Ok(format!("{w}*{h}"))
}

fn timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 执行一次出图。返回给模型的文本：逐行 `![generated](绝对路径)` ——
/// 前端据此在工具卡里把图渲染出来（见 main.ts 的 ImageGen 分支），
/// 同时这行对人类也是可读的路径。
pub fn run(input: &Value) -> Result<String, String> {
    let prompt = input
        .get("prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or("missing required parameter \"prompt\"")?;
    let model = env::var("LUNAC_IMAGE_MODEL").unwrap_or_default();
    if model.trim().is_empty() {
        return Err("ImageGen is not configured (no image model in Lunac settings)".into());
    }
    let key = api_key()?;
    let size = parse_size(input.get("size").and_then(Value::as_str))?;
    let n = input.get("n").and_then(Value::as_u64).unwrap_or(1).clamp(1, MAX_N);
    let enhance = input.get("enhance").and_then(Value::as_bool).unwrap_or(false);
    let neg = input
        .get("negative_prompt")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or(DEFAULT_NEG);

    // 输入图（可选）：1..=3 张。**多给就报错**，不静默截断 —— 静默丢图会让用户
    // 拿到一个「怎么改都不像」的结果却无从知道少了参考图。
    let mut content: Vec<Value> = Vec::new();
    if let Some(arr) = input.get("images").and_then(Value::as_array) {
        let paths: Vec<&str> = arr.iter().filter_map(Value::as_str).map(str::trim).filter(|s| !s.is_empty()).collect();
        if paths.len() > MAX_INPUT_IMAGES {
            return Err(format!(
                "at most {MAX_INPUT_IMAGES} input images per call (got {})",
                paths.len()
            ));
        }
        for p in &paths {
            content.push(json!({ "image": data_uri(p)? }));
        }
    }
    content.push(json!({ "text": prompt }));

    let body = json!({
        "model": model,
        "input": { "messages": [ { "role": "user", "content": content } ] },
        "parameters": {
            "n": n,
            "watermark": false,
            "prompt_extend": enhance,
            "size": size,
            "negative_prompt": neg,
        }
    });

    crate::log::info(format!(
        "ImageGen: model={model} size={size} n={n} inputs={} enhance={enhance}",
        content.len() - 1
    ));

    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(GEN_TIMEOUT_SECS))
        .build()
        .map_err(|e| format!("build http client: {e}"))?;
    let resp = client
        .post(endpoint())
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .json(&body)
        .send()
        .map_err(|e| format!("image request failed: {e}"))?;

    let status = resp.status();
    let text = resp.text().map_err(|e| format!("read image response: {e}"))?;
    let parsed: Value = serde_json::from_str(&text)
        .map_err(|_| format!("image API returned non-JSON (HTTP {status}): {}", crate::log::truncate_chars(&text, 500)))?;
    // 业务错误也走 HTTP 200 + `code` 字段（见 qwen_edit.py 的判据）
    if let Some(code) = parsed.get("code").and_then(Value::as_str) {
        let msg = parsed.get("message").and_then(Value::as_str).unwrap_or("");
        return Err(format!("image API error: {code} / {}", crate::log::truncate_chars(msg, 400)));
    }
    if !status.is_success() {
        return Err(format!(
            "image API HTTP {status}: {}",
            crate::log::truncate_chars(&text, 500)
        ));
    }

    let urls: Vec<String> = parsed
        .get("output")
        .and_then(|o| o.get("choices"))
        .and_then(Value::as_array)
        .and_then(|c| c.first())
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|it| it.get("image").and_then(Value::as_str).map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    if urls.is_empty() {
        return Err(format!(
            "image API returned no image: {}",
            crate::log::truncate_chars(&text, 500)
        ));
    }

    // 下载落盘。**逐张记结果**：某张下载失败不该把已经拿到的其它张一起丢掉。
    let dir = image_dir();
    fs::create_dir_all(&dir).map_err(|e| format!("create images dir {}: {e}", dir.display()))?;
    let stamp = timestamp();
    let mut lines: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    for (i, url) in urls.iter().enumerate() {
        match download(&client, url) {
            Ok(bytes) => {
                let path = dir.join(format!("img_{stamp}_{}.{}", i + 1, ext_for(&bytes)));
                match fs::write(&path, &bytes) {
                    Ok(()) => lines.push(format!("![generated]({})", path.display())),
                    Err(e) => failures.push(format!("save failed: {e}")),
                }
            }
            Err(e) => failures.push(e),
        }
    }
    if lines.is_empty() {
        return Err(format!("all {} image(s) failed to download: {}", urls.len(), failures.join("; ")));
    }

    let mut out = format!(
        "Generated {} image(s) with {model} at {size}:\n{}",
        lines.len(),
        lines.join("\n")
    );
    if !failures.is_empty() {
        out.push_str(&format!("\n(partial failures: {})", failures.join("; ")));
    }
    Ok(out)
}

fn download(client: &reqwest::blocking::Client, url: &str) -> Result<Vec<u8>, String> {
    let resp = client.get(url).send().map_err(|e| format!("download failed: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("download HTTP {}", resp.status()));
    }
    resp.bytes().map(|b| b.to_vec()).map_err(|e| format!("download body: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_is_validated_against_the_512_2048_window() {
        assert_eq!(parse_size(None).unwrap(), "1024*1024");
        assert_eq!(parse_size(Some("1024*1536")).unwrap(), "1024*1536");
        // `x` 也认（用户手写常见），但发出去一律规范化成 `*`
        assert_eq!(parse_size(Some("768x768")).unwrap(), "768*768");
        assert!(parse_size(Some("256*256")).is_err(), "低于 512 要拒");
        assert!(parse_size(Some("4096*4096")).is_err(), "高于 2048 要拒");
        assert!(parse_size(Some("1024")).is_err(), "缺一边要拒");
        assert!(parse_size(Some("abc*def")).is_err());
    }

    #[test]
    fn media_type_uses_magic_bytes_not_extensions() {
        assert_eq!(media_type(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]), Some("image/png"));
        assert_eq!(media_type(&[0xFF, 0xD8, 0xFF, 0xE0]), Some("image/jpeg"));
        assert_eq!(media_type(b"GIF89a....."), Some("image/gif"));
        assert_eq!(media_type(b"not an image"), None);
    }
}

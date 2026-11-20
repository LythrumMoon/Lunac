//! 插件开发工具链（轨道 H；v1 `plugin_toolchain.rs` 的重建）。
//!
//! **契约来源**：`agent-templates/modules/README.md` §开发工具链（2026-10-18 定稿）。
//! 本篇逐条对齐它，不另立字段：
//!   · `dev.toolchains[]` = **可下载**的 profile（`id / version / url / sha256 / kind /
//!     dest / binDir`），最多 2 条；`dest` / `binDir` 必须在 `.toolchain\` 下；
//!   · `dev.build` = `{ command, args, cwd }`；`command` 是**包内相对路径**或白名单裸命令；
//!   · 授权键 = `(插件 id, dev 段指纹)`，**改任何一个 dev 字段旧授权立即失效**；
//!   · 永不自动化：装插件只登记声明，下载工具链 / 跑构建都要用户点；
//!   · **裸命令只从工具链 `binDir` 解析，不回落系统 `PATH`**；
//!   · 撤销 = 删授权 + 立刻杀掉构建进程（可选删 `.toolchain\`）。
//!
//! 诚实边界（UI 与文档必须原样保留）：**这不是沙箱** —— 构建进程能读写用户有权访问的
//! 一切；工具链与资源闸只是「可撤销 + 可限额」。

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

use crate::plugin_market::{self, BuildSpec, DevSpec, PluginManifest, ToolchainProfile};

/// 授权账本的版本（变了就拒绝读取，不猜测迁移）。
const LEDGER_VERSION: u64 = 1;
/// 单个工具链下载 / 解压的上限（1 GiB）。
const MAX_DOWNLOAD_BYTES: u64 = 1024 * 1024 * 1024;
/// 构建的默认超时（README 没有超时字段；600 秒够任何本地打包）。
const BUILD_TIMEOUT_SEC: u64 = 600;
/// 每个子进程输出保留的**尾部**字节数（只用于回显，不落盘）。
const MAX_OUTPUT_BYTES: usize = 8 * 1024;
/// 下载超时（单次请求）。
const DOWNLOAD_TIMEOUT_SEC: u64 = 900;
/// 进程级复用的 HTTP 客户端（与 `skills_admin` / `translate` 同范式）。
static HTTP_CLIENT: LazyLock<Result<reqwest::blocking::Client, String>> = LazyLock::new(|| {
    reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(DOWNLOAD_TIMEOUT_SEC))
        .build()
        .map_err(|e| {
            eprintln!("[lunac-toolchain] 初始化 HTTP 客户端失败: {e}");
            "网络组件初始化失败，请稍后再试".to_string()
        })
});

/// 正在跑的构建（同一插件同时只允许一个）。
///
/// 存 `child_job::Job` 就够了：它的 `Drop` 会 `CloseHandle` ⇒ `KILL_ON_JOB_CLOSE`
/// 把整棵进程树收掉 ——「停止」= 删掉这个槽位。
struct BuildSlot {
    /// 不读取，靠 `Drop` 关闭 Job Object 来回收整棵构建进程树。
    #[allow(dead_code)]
    job: crate::child_job::Job,
}

static RUNNING: LazyLock<Mutex<HashMap<String, BuildSlot>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

fn ledger_path(root: &Path) -> PathBuf {
    root.join("config").join("plugin-toolchain.json")
}

/// 读账本；文件不存在 = 空账本（首次授权），**读坏 / 版本不符 ⇒ Err**（不静默清空）。
fn read_ledger(root: &Path) -> Result<Value, String> {
    let path = ledger_path(root);
    if !path.exists() {
        return Ok(json!({ "version": LEDGER_VERSION, "grants": {} }));
    }
    let text = std::fs::read_to_string(&path)
        .map_err(|e| format!("读不了授权记录（{}）：{e}", path.display()))?;
    let value: Value = serde_json::from_str(&text)
        .map_err(|e| format!("授权记录不是合法 JSON（{}）：{e}", path.display()))?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version != LEDGER_VERSION {
        return Err(format!(
            "授权记录版本是 {version}（本版只认 {LEDGER_VERSION}）；删掉 {} 重新授权即可",
            path.display()
        ));
    }
    if !value.get("grants").map(Value::is_object).unwrap_or(false) {
        return Err(format!(
            "授权记录缺少 grants 对象（{}）；删掉它重新授权即可",
            path.display()
        ));
    }
    Ok(value)
}

fn write_ledger(root: &Path, value: &Value) -> Result<(), String> {
    let path = ledger_path(root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("建不了配置目录（{}）：{e}", parent.display()))?;
    }
    let text = serde_json::to_string_pretty(value).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)
        .map_err(|e| format!("写不进 {}：{e}；请确认目录可写", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("替换 {} 失败：{e}", path.display()))?;
    Ok(())
}

/// `dev` 段指纹：`serde_json` 的规范化序列化再 sha256。
///
/// 只覆盖 `dev` 段（README 的口径：profile 的字段 + `build` 的字段），所以改描述 / 图标
/// 不会白废授权，改工具链或构建命令一定废。
fn dev_fingerprint(dev: &Option<DevSpec>) -> String {
    let text = serde_json::to_string(dev).unwrap_or_default();
    let mut hasher = Sha256::new();
    hasher.update(text.as_bytes());
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

struct PluginSource {
    dir: PathBuf,
    manifest: PluginManifest,
    fingerprint: String,
}

fn load_plugin(root: &Path, plugin: &str) -> Result<PluginSource, String> {
    let plugin = plugin.trim();
    if plugin.is_empty() {
        return Err("缺少插件 id".to_string());
    }
    // `resolve_plugin_dir` 的 root 是**插件根**（`<exe 根>\Modules`），不是 exe 根。
    let dir = plugin_market::resolve_plugin_dir(&plugin_market::plugins_dir(root), plugin)
        .ok_or_else(|| format!("没找到插件目录：{plugin}（先在插件市场装一次）"))?;
    let manifest_path = dir.join("lunac-plugin.json");
    let text = std::fs::read_to_string(&manifest_path)
        .map_err(|e| format!("读不了 {}：{e}", manifest_path.display()))?;
    let manifest = plugin_market::parse_manifest(&text)?;
    let fingerprint = dev_fingerprint(&manifest.dev);
    Ok(PluginSource { dir, manifest, fingerprint })
}

fn dev_of(source: &PluginSource) -> DevSpec {
    source.manifest.dev.clone().unwrap_or_default()
}

fn toolchain_root(source: &PluginSource) -> PathBuf {
    source.dir.join(".toolchain")
}

fn marker_path(source: &PluginSource, id: &str) -> PathBuf {
    toolchain_root(source).join(format!("{id}.json"))
}

/// 装好了吗：标记存在、哈希对得上、落点还在。
///
/// **哈希对得上**是关键：清单里换了工具链版本（sha256 变）⇒ 旧目录不再算「已安装」，
/// 必须重新下载。
fn installed(source: &PluginSource, profile: &ToolchainProfile) -> bool {
    let Ok(text) = std::fs::read_to_string(marker_path(source, &profile.id)) else {
        return false;
    };
    let Ok(marker) = serde_json::from_str::<Value>(&text) else {
        return false;
    };
    let same = marker.get("sha256").and_then(Value::as_str) == Some(profile.sha256.trim());
    let dest = source.dir.join(profile.dest.trim());
    same && dest.exists()
}

fn dev_granted(source: &PluginSource, ledger: &Value) -> bool {
    ledger
        .get("grants")
        .and_then(Value::as_object)
        .and_then(|grants| grants.get(&source.manifest.id))
        .and_then(|entry| entry.get("fingerprint"))
        .and_then(Value::as_str)
        == Some(source.fingerprint.as_str())
}

fn is_running(plugin: &str) -> bool {
    RUNNING
        .lock()
        .map(|map| map.contains_key(plugin))
        .unwrap_or(false)
}

// ── 对外入口（JSON 进 / JSON 出，FFI 直连）────────────────────────

/// `{ plugin }` → 状态：`{ ok, plugin, name, fingerprint, toolchains[], build, running,
/// granted, error }`。
pub fn status_json(root: &Path, request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("");
    let source = match load_plugin(root, plugin) {
        Ok(value) => value,
        Err(reason) => return json!({ "ok": false, "plugin": plugin, "error": reason }).to_string(),
    };
    let ledger = match read_ledger(root) {
        Ok(value) => value,
        Err(reason) => {
            return json!({ "ok": false, "plugin": plugin, "error": reason }).to_string()
        }
    };
    let dev = dev_of(&source);
    let toolchains: Vec<Value> = dev
        .toolchains
        .iter()
        .map(|profile| {
            json!({
                "id": profile.id,
                "version": profile.version,
                "url": profile.url,
                "sha256": profile.sha256,
                "kind": if profile.kind.trim().is_empty() { "file" } else { profile.kind.trim() },
                "dest": profile.dest,
                "binDir": profile.bin_dir,
                "installed": installed(&source, profile),
            })
        })
        .collect();
    json!({
        "ok": true,
        "plugin": source.manifest.id,
        "name": source.manifest.name,
        "fingerprint": source.fingerprint,
        "toolchains": toolchains,
        "build": dev.build.map(|build| json!({
            "command": build.command,
            "args": build.args,
            "cwd": build.cwd,
            "label": build_label(&build),
        })),
        "running": is_running(&source.manifest.id),
        "granted": dev_granted(&source, &ledger),
        "error": "",
    })
    .to_string()
}

fn build_label(build: &BuildSpec) -> String {
    let mut text = build.command.trim().to_string();
    for arg in &build.args {
        text.push(' ');
        text.push_str(arg);
    }
    text
}

/// 授权整个 `dev` 段（README：授权键 = 插件 id + dev 段指纹）。
pub fn grant_json(root: &Path, request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("");
    let source = match load_plugin(root, plugin) {
        Ok(value) => value,
        Err(reason) => return action_error(reason),
    };
    let dev = dev_of(&source);
    if dev.toolchains.is_empty() && dev.build.is_none() {
        return action_error("这个插件没有 dev 段：没有可授权的工具链 / 构建".to_string());
    }
    let mut ledger = match read_ledger(root) {
        Ok(value) => value,
        Err(reason) => return action_error(reason),
    };
    let entry = json!({
        "plugin": source.manifest.id,
        "fingerprint": source.fingerprint,
        "grantedAt": now_ms(),
    });
    let Some(grants) = ledger.get_mut("grants").and_then(Value::as_object_mut) else {
        return action_error("授权记录缺少 grants 对象".to_string());
    };
    grants.insert(source.manifest.id.clone(), entry);
    if let Err(reason) = write_ledger(root, &ledger) {
        return action_error(reason);
    }
    audit(&format!(
        "toolchain: grant {}（dev 指纹 {}）",
        source.manifest.id,
        short(&source.fingerprint)
    ));
    json!({ "ok": true, "error": "" }).to_string()
}

/// `{ plugin, deleteToolchain? }` → 删授权 + 停构建（可选删 `.toolchain\`）。
pub fn revoke_json(root: &Path, request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("");
    let delete_toolchain = request
        .get("deleteToolchain")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let source = match load_plugin(root, plugin) {
        Ok(value) => value,
        Err(reason) => return action_error(reason),
    };
    let mut ledger = match read_ledger(root) {
        Ok(value) => value,
        Err(reason) => return action_error(reason),
    };
    let removed = ledger
        .get_mut("grants")
        .and_then(Value::as_object_mut)
        .and_then(|grants| grants.remove(&source.manifest.id))
        .is_some();
    if let Err(reason) = write_ledger(root, &ledger) {
        return action_error(reason);
    }
    // 撤销必须**立刻**停掉在跑的构建（README 的硬要求）。
    let stopped = stop_running(&source.manifest.id);
    let mut deleted = false;
    if delete_toolchain {
        let dir = toolchain_root(&source);
        if dir.exists() {
            match std::fs::remove_dir_all(&dir) {
                Ok(()) => deleted = true,
                Err(error) => {
                    return json!({
                        "ok": false,
                        "removed": removed,
                        "stopped": stopped,
                        "deleted": false,
                        "error": format!("删不掉 {}：{error}", dir.display()),
                    })
                    .to_string()
                }
            }
        }
    }
    audit(&format!(
        "toolchain: revoke {}{}{}",
        source.manifest.id,
        if stopped { "（已停构建）" } else { "" },
        if deleted { "（已删 .toolchain）" } else { "" }
    ));
    json!({
        "ok": true,
        "removed": removed,
        "stopped": stopped,
        "deleted": deleted,
        "error": "",
    })
    .to_string()
}

/// `{ plugin }` → 下载并安装全部工具链（需已授权；同步，前端用 `Isolate.run`）。
pub fn install_json(root: &Path, request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("");
    let source = match load_plugin(root, plugin) {
        Ok(value) => value,
        Err(reason) => return install_error(&[], reason),
    };
    let ledger = match read_ledger(root) {
        Ok(value) => value,
        Err(reason) => return install_error(&[], reason),
    };
    if !dev_granted(&source, &ledger) {
        return install_error(&[], "这个插件还没有授权：先在信任卡里点「授权」".to_string());
    }
    let dev = dev_of(&source);
    if dev.toolchains.is_empty() {
        return install_error(&[], "这个插件没有声明工具链（dev.toolchains 为空）".to_string());
    }
    let client = match HTTP_CLIENT.as_ref() {
        Ok(client) => client.clone(),
        Err(reason) => return install_error(&[], reason.clone()),
    };
    let mut steps: Vec<Value> = Vec::new();
    for profile in &dev.toolchains {
        match install_one(&client, &source, profile) {
            Ok(bytes) => {
                steps.push(json!({
                    "id": profile.id,
                    "ok": true,
                    "bytes": bytes,
                    "dest": profile.dest,
                }));
                audit(&format!(
                    "toolchain: install {} / {}（{} 字节）",
                    source.manifest.id,
                    profile.id,
                    bytes
                ));
            }
            Err(reason) => {
                steps.push(json!({ "id": profile.id, "ok": false, "bytes": 0 }));
                return install_error(&steps, format!("{}：{reason}", profile.id));
            }
        }
    }
    json!({
        "ok": true,
        "plugin": source.manifest.id,
        "steps": steps,
        "error": "",
    })
    .to_string()
}

/// 下载一个 profile → 校验 sha256 → 落盘 → 写标记。
fn install_one(
    client: &reqwest::blocking::Client,
    source: &PluginSource,
    profile: &ToolchainProfile,
) -> Result<u64, String> {
    let root = toolchain_root(source);
    std::fs::create_dir_all(&root)
        .map_err(|e| format!("建不了 {}：{e}", root.display()))?;
    let temp = root.join(format!(".download-{}.tmp", profile.id));
    let bytes = download_to(client, profile.url.trim(), &temp)?;
    let actual = file_sha256(&temp)?;
    let want = profile.sha256.trim().to_ascii_lowercase();
    if actual != want {
        let _ = std::fs::remove_file(&temp);
        return Err(format!(
            "哈希不符（清单写 {}，下载到 {}）：已丢弃，请不要使用这个来源",
            short(&want),
            short(&actual)
        ));
    }
    let dest = source.dir.join(profile.dest.trim());
    if profile.kind.trim() == "archive" {
        let staging = root.join(format!(".staging-{}", profile.id));
        if staging.exists() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        let file = std::fs::File::open(&temp).map_err(|e| format!("读不了下载文件：{e}"))?;
        plugin_market::extract_zip(std::io::BufReader::new(file), &staging, MAX_DOWNLOAD_BYTES)
            .map_err(|e| format!("解压失败：{e}"))?;
        let _ = std::fs::remove_file(&temp);
        if dest.exists() {
            std::fs::remove_dir_all(&dest)
                .map_err(|e| format!("替换旧工具链失败（{}）：{e}", dest.display()))?;
        }
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建不了 {}：{e}", parent.display()))?;
        }
        std::fs::rename(&staging, &dest)
            .map_err(|e| format!("移动工具链到 {} 失败：{e}", dest.display()))?;
    } else {
        if let Some(parent) = dest.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("建不了 {}：{e}", parent.display()))?;
        }
        if dest.exists() {
            let _ = std::fs::remove_file(&dest);
        }
        std::fs::rename(&temp, &dest)
            .map_err(|e| format!("移动工具链到 {} 失败：{e}", dest.display()))?;
    }
    let marker = json!({
        "id": profile.id,
        "version": profile.version,
        "sha256": profile.sha256,
        "dest": profile.dest,
        "kind": if profile.kind.trim().is_empty() { "file" } else { profile.kind.trim() },
        "bytes": bytes,
        "installedAt": now_ms(),
    });
    std::fs::write(marker_path(source, &profile.id), marker.to_string())
        .map_err(|e| format!("写不进安装标记：{e}"))?;
    Ok(bytes)
}

/// 流式下载到 `path`，边写边卡上限（不等下完再判大小）。
fn download_to(client: &reqwest::blocking::Client, url: &str, path: &Path) -> Result<u64, String> {
    plugin_market::validate_public_https_url(url)?;
    let mut response = client.get(url).send().map_err(|e| {
        eprintln!("[lunac-toolchain] 下载失败 {url}: {e}");
        "下载失败：连不上这个地址，请确认网络与链接".to_string()
    })?;
    if !response.status().is_success() {
        eprintln!("[lunac-toolchain] 下载返回 {}：{url}", response.status());
        return Err(format!("下载失败（服务器返回 {}）", response.status()));
    }
    if let Some(length) = response.content_length() {
        if length > MAX_DOWNLOAD_BYTES {
            return Err(oversize(length));
        }
    }
    let mut file = std::fs::File::create(path)
        .map_err(|e| format!("写不进临时文件（{}）：{e}", path.display()))?;
    let mut written: u64 = 0;
    let mut chunk = [0u8; 64 * 1024];
    use std::io::Write;
    loop {
        let read = response
            .read(&mut chunk)
            .map_err(|e| format!("下载中断：{e}"))?;
        if read == 0 {
            break;
        }
        written += read as u64;
        if written > MAX_DOWNLOAD_BYTES {
            drop(file);
            let _ = std::fs::remove_file(path);
            return Err(oversize(written));
        }
        file.write_all(&chunk[..read])
            .map_err(|e| format!("写不进临时文件：{e}"))?;
    }
    file.flush().map_err(|e| format!("落盘失败：{e}"))?;
    Ok(written)
}

fn oversize(bytes: u64) -> String {
    format!(
        "工具链太大（{} MiB）：单条上限 {} MiB",
        bytes / (1024 * 1024),
        MAX_DOWNLOAD_BYTES / (1024 * 1024)
    )
}

fn file_sha256(path: &Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("读不了 {}：{e}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut chunk).map_err(|e| format!("读不了下载文件：{e}"))?;
        if read == 0 {
            break;
        }
        hasher.update(&chunk[..read]);
    }
    let mut out = String::with_capacity(64);
    for byte in hasher.finalize() {
        out.push_str(&format!("{byte:02x}"));
    }
    Ok(out)
}

/// `{ plugin }` → 跑 `dev.build`（需已授权；**永不自动触发**）。
pub fn build_json(root: &Path, request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("");
    let source = match load_plugin(root, plugin) {
        Ok(value) => value,
        Err(reason) => return build_error(reason),
    };
    let ledger = match read_ledger(root) {
        Ok(value) => value,
        Err(reason) => return build_error(reason),
    };
    if !dev_granted(&source, &ledger) {
        return build_error("这个插件还没有授权：先授权再构建".to_string());
    }
    let dev = dev_of(&source);
    let Some(build) = dev.build.clone() else {
        return build_error("这个插件没有声明 dev.build（没有可运行的构建命令）".to_string());
    };
    let id = source.manifest.id.clone();
    if is_running(&id) {
        return build_error("这个插件已有构建在跑：先停掉再试".to_string());
    }
    // 裸命令只从**已安装**的工具链 binDir 解析（README 的硬约束：不回落系统 PATH）。
    let program = match resolve_program(&source, &dev, &build) {
        Ok(path) => path,
        Err(reason) => return build_error(reason),
    };
    let Some(job) = crate::child_job::Job::create(crate::child_job::JobLimits::default()) else {
        return build_error("建不了进程组（Job Object）：构建已取消".to_string());
    };
    if let Ok(mut map) = RUNNING.lock() {
        map.insert(id.clone(), BuildSlot { job });
    }
    let outcome = run_build(&source, &dev, &build, &program);
    let stopped = !is_running(&id);
    if let Ok(mut map) = RUNNING.lock() {
        map.remove(&id);
    }
    match outcome {
        Ok(run) => {
            let ok = !run.timed_out && run.code == Some(0) && !stopped;
            audit(&format!(
                "toolchain: build {id} `{}` → {:?}（{} ms）{}",
                clip(&build_label(&build)),
                run.code,
                run.duration_ms,
                if stopped { "【已停止】" } else { "" }
            ));
            json!({
                "ok": ok,
                "plugin": id,
                "command": build_label(&build),
                "code": run.code,
                "durationMs": run.duration_ms,
                "timedOut": run.timed_out,
                "stopped": stopped,
                "output": run.output,
                "error": if ok { String::new() } else if stopped {
                    "构建被用户停止".to_string()
                } else if run.timed_out {
                    format!("构建超时（{BUILD_TIMEOUT_SEC} 秒）")
                } else {
                    format!("构建失败（退出码 {:?}）", run.code)
                },
            })
            .to_string()
        }
        Err(reason) => build_error(reason),
    }
}

/// `{ plugin }` → 停掉在跑的构建（整棵进程树）。
pub fn stop_json(request: &Value) -> String {
    let plugin = request.get("plugin").and_then(Value::as_str).unwrap_or("").trim();
    if plugin.is_empty() {
        return action_error("缺少插件 id".to_string());
    }
    let stopped = stop_running(plugin);
    if stopped {
        audit(&format!("toolchain: stop {plugin}"));
    }
    json!({ "ok": true, "stopped": stopped, "error": "" }).to_string()
}

fn stop_running(plugin: &str) -> bool {
    let slot = RUNNING.lock().ok().and_then(|mut map| map.remove(plugin));
    if let Some(slot) = slot {
        drop(slot);
        return true;
    }
    false
}

/// 解析要执行的程序：包内相对路径原样用；裸命令只在已装好的工具链 binDir 里找。
fn resolve_program(
    source: &PluginSource,
    dev: &DevSpec,
    build: &BuildSpec,
) -> Result<PathBuf, String> {
    let command = build.command.trim();
    let relative = command.contains('/') || command.contains('\\') || command.starts_with('.');
    if relative {
        let Some(rel) = plugin_market::dev_relative(command) else {
            return Err(format!("build.command 越界：{command}"));
        };
        let path = source.dir.join(rel);
        if !path.exists() {
            return Err(format!("build.command 指向的文件不存在：{command}"));
        }
        return Ok(path);
    }
    if !plugin_market::DEV_BARE_COMMANDS.contains(&command) {
        return Err(format!(
            "build.command 只允许包内相对路径或白名单裸命令（{command} 不在白名单）"
        ));
    }
    let mut searched: Vec<String> = Vec::new();
    for profile in &dev.toolchains {
        if !installed(source, profile) {
            continue;
        }
        let bin = profile.bin_dir.trim();
        if bin.is_empty() {
            continue;
        }
        let Some(rel) = plugin_market::dev_relative(bin) else {
            continue;
        };
        let dir = source.dir.join(rel);
        searched.push(dir.to_string_lossy().to_string());
        for name in candidate_names(command) {
            let path = dir.join(name);
            if path.is_file() {
                return Ok(path);
            }
        }
    }
    Err(format!(
        "工具链里找不到「{command}」：先装工具链（裸命令只从工具链 binDir 解析，不回落系统 PATH）。已查找：{}",
        if searched.is_empty() { "（没有已安装的 binDir）".to_string() } else { searched.join("；") }
    ))
}

fn candidate_names(command: &str) -> Vec<String> {
    vec![
        format!("{command}.exe"),
        format!("{command}.cmd"),
        format!("{command}.bat"),
        command.to_string(),
    ]
}

struct RunOutcome {
    code: Option<i32>,
    timed_out: bool,
    duration_ms: u64,
    output: String,
}

fn run_build(
    source: &PluginSource,
    dev: &DevSpec,
    build: &BuildSpec,
    program: &Path,
) -> Result<RunOutcome, String> {
    let cwd = if build.cwd.trim().is_empty() {
        source.dir.clone()
    } else {
        let Some(rel) = plugin_market::dev_relative(build.cwd.trim()) else {
            return Err(format!("build.cwd 越界：{}", build.cwd));
        };
        source.dir.join(rel)
    };
    // PATH 只放已装工具链的 binDir（README：不回落系统 PATH）。
    let path_env = build_path(source, dev);
    let is_script = matches!(
        program
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.to_ascii_lowercase())
            .as_deref(),
        Some("cmd") | Some("bat")
    );
    let mut command = if is_script {
        // `.cmd` / `.bat` 只能经 cmd.exe 执行；参数里带 shell 元字符就拒绝
        // （cmd 会展开它们 —— 那是注入面，不假装「不经 shell」）。
        for arg in &build.args {
            if arg.contains(['&', '|', '<', '>', '^', '%', '"', '!']) {
                return Err(format!(
                    "参数里不能有 cmd 元字符（& | < > ^ % \" !）：{arg}；请换一个不经过 .cmd 的命令"
                ));
            }
        }
        let mut cmd = Command::new("cmd");
        cmd.arg("/C").arg(program);
        cmd
    } else {
        Command::new(program)
    };
    command
        .args(&build.args)
        .current_dir(&cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if !path_env.is_empty() {
        command.env("PATH", path_env);
    }
    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW：构建不该在用户桌面上闪黑框。
        command.creation_flags(0x0800_0000);
    }
    let started = Instant::now();
    let mut child = command
        .spawn()
        .map_err(|e| format!("起不了 {}：{e}", program.display()))?;
    crate::child_job::assign(&child);
    let out_reader = child.stdout.take().map(spawn_reader);
    let err_reader = child.stderr.take().map(spawn_reader);
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) => {}
            Err(_) => break None,
        }
        if started.elapsed() > Duration::from_secs(BUILD_TIMEOUT_SEC) {
            timed_out = true;
            let _ = child.kill();
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(80));
    };
    let mut output = String::new();
    if let Some(handle) = out_reader {
        if let Ok(text) = handle.join() {
            output.push_str(&text);
        }
    }
    if let Some(handle) = err_reader {
        if let Ok(text) = handle.join() {
            if !text.is_empty() {
                if !output.is_empty() {
                    output.push('\n');
                }
                output.push_str(&text);
            }
        }
    }
    Ok(RunOutcome {
        code: status.and_then(|s| s.code()),
        timed_out,
        duration_ms: started.elapsed().as_millis() as u64,
        output,
    })
}

fn build_path(source: &PluginSource, dev: &DevSpec) -> String {
    let mut bins: Vec<String> = Vec::new();
    for profile in &dev.toolchains {
        if !installed(source, profile) {
            continue;
        }
        let bin = profile.bin_dir.trim();
        if bin.is_empty() {
            continue;
        }
        if let Some(rel) = plugin_market::dev_relative(bin) {
            bins.push(source.dir.join(rel).to_string_lossy().to_string());
        }
    }
    bins.join(";")
}

/// 读一个管道，只保留**尾部** `MAX_OUTPUT_BYTES`（构建日志真正有用的是结尾）。
fn spawn_reader<R: Read + Send + 'static>(mut reader: R) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let mut buffer: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            match reader.read(&mut chunk) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    buffer.extend_from_slice(&chunk[..read]);
                    if buffer.len() > MAX_OUTPUT_BYTES {
                        let cut = buffer.len() - MAX_OUTPUT_BYTES;
                        buffer.drain(..cut);
                    }
                }
            }
        }
        String::from_utf8_lossy(&buffer).to_string()
    })
}

fn action_error(reason: String) -> String {
    json!({ "ok": false, "error": reason }).to_string()
}

fn install_error(steps: &[Value], reason: String) -> String {
    json!({
        "ok": false,
        "steps": steps,
        "error": reason,
    })
    .to_string()
}

fn build_error(reason: String) -> String {
    json!({
        "ok": false,
        "code": null,
        "durationMs": 0,
        "timedOut": false,
        "stopped": false,
        "output": "",
        "error": reason,
    })
    .to_string()
}

/// 审计行 = 既有日志（`<exe 根>\logs\`），只记命令与结论；命令文本过 `mask_secrets()`。
fn audit(message: &str) {
    crate::log::info(crate::security_ledger::mask_secrets(message));
}

fn clip(text: &str) -> String {
    text.chars().take(160).collect()
}

fn short(text: &str) -> &str {
    &text[..text.len().min(8)]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-toolchain-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn manifest_text(id: &str, dev: &str) -> String {
        format!(r#"{{"id":"{id}","name":"{id}","version":"1.0.0","entry":"index.js","dev":{dev}}}"#)
    }

    fn write_plugin(root: &Path, id: &str, dev: &str) {
        let dir = root.join("Modules").join(id);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lunac-plugin.json"), manifest_text(id, dev)).unwrap();
        std::fs::write(dir.join("index.js"), "export default {}").unwrap();
    }

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// 默认 dev 段：一个 node 工具链 + 一条构建命令（符合 README 的字段）。
    fn dev() -> String {
        format!(
            r#"{{
                "toolchains": [{{
                    "id": "node",
                    "version": "22.14.0",
                    "url": "https://nodejs.org/dist/v22.14.0/node-v22.14.0-win-x64.zip",
                    "sha256": "{SHA}",
                    "kind": "archive",
                    "dest": ".toolchain/node",
                    "binDir": ".toolchain/node/node-v22.14.0-win-x64"
                }}],
                "build": {{ "command": "npm", "args": ["run", "build"], "cwd": "." }}
            }}"#
        )
    }

    fn status(root: &Path, id: &str) -> Value {
        serde_json::from_str(&status_json(root, &json!({ "plugin": id }))).expect("json")
    }

    #[test]
    fn grant_and_revoke_round_trip() {
        let root = temp_root("grant");
        write_plugin(&root, "tool-demo", &dev());
        let before = status(&root, "tool-demo");
        assert_eq!(before["ok"], true);
        assert_eq!(before["granted"], false);
        assert_eq!(before["toolchains"][0]["id"], "node");
        assert_eq!(before["toolchains"][0]["installed"], false);
        assert_eq!(before["toolchains"][0]["kind"], "archive");
        assert_eq!(before["build"]["label"], "npm run build");

        let granted: Value = serde_json::from_str(&grant_json(
            &root,
            &json!({ "plugin": "tool-demo" }),
        ))
        .expect("json");
        assert_eq!(granted["ok"], true, "{}", granted["error"]);
        assert_eq!(status(&root, "tool-demo")["granted"], true);

        let revoked: Value = serde_json::from_str(&revoke_json(
            &root,
            &json!({ "plugin": "tool-demo" }),
        ))
        .expect("json");
        assert_eq!(revoked["ok"], true);
        assert_eq!(revoked["removed"], true);
        assert_eq!(status(&root, "tool-demo")["granted"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 改 dev 段任何一个字段 ⇒ 指纹变 ⇒ 旧授权立即失效；只改描述不影响。
    #[test]
    fn changing_the_dev_section_invalidates_the_grant() {
        let root = temp_root("fingerprint");
        write_plugin(&root, "tool-demo", &dev());
        let _: Value =
            serde_json::from_str(&grant_json(&root, &json!({ "plugin": "tool-demo" }))).unwrap();
        assert_eq!(status(&root, "tool-demo")["granted"], true);

        // 只改描述：指纹不变。
        let dir = root.join("Modules").join("tool-demo");
        let text = manifest_text("tool-demo", &dev()).replace("\"name\":\"tool-demo\"", "\"name\":\"改名\"");
        std::fs::write(dir.join("lunac-plugin.json"), text).unwrap();
        assert_eq!(status(&root, "tool-demo")["granted"], true, "改描述不该废授权");

        // 改 build.args：指纹变。
        let text = manifest_text("tool-demo", &dev()).replace("\"run\", \"build\"", "\"run\", \"build2\"");
        std::fs::write(dir.join("lunac-plugin.json"), text).unwrap();
        let after = status(&root, "tool-demo");
        assert_eq!(after["granted"], false, "改构建命令必须重新授权");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn install_and_build_require_a_grant() {
        let root = temp_root("gate");
        write_plugin(&root, "tool-demo", &dev());
        let installed: Value = serde_json::from_str(&install_json(
            &root,
            &json!({ "plugin": "tool-demo" }),
        ))
        .expect("json");
        assert_eq!(installed["ok"], false);
        assert!(installed["error"].as_str().unwrap().contains("授权"));

        let built: Value =
            serde_json::from_str(&build_json(&root, &json!({ "plugin": "tool-demo" }))).unwrap();
        assert_eq!(built["ok"], false);
        assert!(built["error"].as_str().unwrap().contains("授权"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_plugin_without_dev_section_is_reported_honestly() {
        let root = temp_root("nodev");
        write_plugin(&root, "plain", "null");
        let value = status(&root, "plain");
        assert_eq!(value["ok"], true);
        assert_eq!(value["toolchains"], json!([]));
        assert_eq!(value["build"], Value::Null);
        assert_eq!(value["granted"], false);
        let granted: Value =
            serde_json::from_str(&grant_json(&root, &json!({ "plugin": "plain" }))).unwrap();
        assert_eq!(granted["ok"], false);
        assert!(granted["error"].as_str().unwrap().contains("dev"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// README 表格里的硬约束：坏声明一律在**解析阶段**拒整包。
    #[test]
    fn unsafe_dev_sections_are_rejected_at_manifest_parse() {
        let cases = [
            // http（不是 https）
            r#"{"toolchains":[{"id":"n","url":"http://x.example/n.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/n"}]}"#,
            // 缺 sha256
            r#"{"toolchains":[{"id":"n","url":"https://x.example/n.zip","dest":".toolchain/n"}]}"#,
            // dest 不在 .toolchain 下
            r#"{"toolchains":[{"id":"n","url":"https://x.example/n.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":"node"}]}"#,
            // binDir 越界
            r#"{"toolchains":[{"id":"n","url":"https://x.example/n.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/n","binDir":".toolchain/../../escape"}]}"#,
            // kind 不认识
            r#"{"toolchains":[{"id":"n","url":"https://x.example/n.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/n","kind":"tar"}]}"#,
            // 超过 2 条
            r#"{"toolchains":[{"id":"a","url":"https://x.example/a.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/a"},{"id":"b","url":"https://x.example/b.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/b"},{"id":"c","url":"https://x.example/c.zip","sha256":"AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA","dest":".toolchain/c"}]}"#,
            // 裸命令不在白名单
            r#"{"build":{"command":"cmd","args":["/c","dir"]}}"#,
            // cwd 越界
            r#"{"build":{"command":"npm","cwd":"../.."}}"#,
            // 包内路径越界
            r#"{"build":{"command":"../evil.exe"}}"#,
        ];
        for dev in cases {
            let text = manifest_text("x", dev);
            assert!(
                plugin_market::parse_manifest(&text).is_err(),
                "必须拒绝：{dev}"
            );
        }
        assert!(plugin_market::parse_manifest(&manifest_text("x", &dev())).is_ok());
    }

    /// 「已安装」= 标记存在 + sha256 对得上 + 落点还在；换了版本就不算装好。
    #[test]
    fn installed_requires_a_matching_sha256_marker() {
        let root = temp_root("marker");
        write_plugin(&root, "tool-demo", &dev());
        let source = load_plugin(&root, "tool-demo").unwrap();
        let profile = dev_of(&source).toolchains[0].clone();
        assert!(!installed(&source, &profile));

        std::fs::create_dir_all(root.join("Modules/tool-demo/.toolchain/node")).unwrap();
        std::fs::write(
            marker_path(&source, "node"),
            json!({ "sha256": profile.sha256 }).to_string(),
        )
        .unwrap();
        assert!(installed(&source, &profile));

        // 清单换版本（sha256 变）⇒ 旧标记不再算装好。
        std::fs::write(
            marker_path(&source, "node"),
            json!({ "sha256": "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" }).to_string(),
        )
        .unwrap();
        assert!(!installed(&source, &profile));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 裸命令只从工具链 binDir 解析：没装工具链就明确报错，不回落系统 PATH。
    #[test]
    fn bare_commands_only_resolve_from_the_toolchain() {
        let root = temp_root("bare");
        write_plugin(&root, "tool-demo", &dev());
        let _: Value =
            serde_json::from_str(&grant_json(&root, &json!({ "plugin": "tool-demo" }))).unwrap();
        let built: Value =
            serde_json::from_str(&build_json(&root, &json!({ "plugin": "tool-demo" }))).unwrap();
        assert_eq!(built["ok"], false);
        let error = built["error"].as_str().unwrap();
        assert!(error.contains("工具链"), "错误要说清是工具链里找不到：{error}");
        assert!(error.contains("不回落系统 PATH"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_corrupt_ledger_is_reported_instead_of_reset() {
        let root = temp_root("corrupt");
        write_plugin(&root, "tool-demo", &dev());
        let path = ledger_path(&root);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        let value = status(&root, "tool-demo");
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains("授权记录"));

        std::fs::write(&path, r#"{"version":9,"grants":{}}"#).unwrap();
        let value = status(&root, "tool-demo");
        assert_eq!(value["ok"], false);
        assert!(value["error"].as_str().unwrap().contains("版本"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn stop_without_a_running_task_is_a_no_op() {
        let root = temp_root("stop");
        write_plugin(&root, "tool-demo", &dev());
        let value: Value =
            serde_json::from_str(&stop_json(&json!({ "plugin": "tool-demo" }))).unwrap();
        assert_eq!(value["ok"], true);
        assert_eq!(value["stopped"], false);
        let _ = std::fs::remove_dir_all(&root);
    }
}

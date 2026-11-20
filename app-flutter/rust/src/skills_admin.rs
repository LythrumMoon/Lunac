//! 技能目录的宿主侧管理（v1「设置 · AI · 技能」组的等价物）。
//!
//! 目录口径与 agent 一致：`<exe 根>/skills/<key>/SKILL.md`
//! （`rust/src/agent.rs` 把 `LUNAC_SKILLS_DIR` 指到这里，`core-agent/src/skills.rs`
//! 把**文件夹名**当作 `key`）。所以本模块只做「列出 / 读取 / 写入 / 删除」，
//! 不解释正文语义 —— frontmatter 只抽界面要显示的 `name` / `description` / `context: fork`。
//!
//! 三道闸（与 `sandbox.rs` 同口径）：
//!   1. `key` 只允许 `[A-Za-z0-9_-]`、≤ 64 字符（挡住 `..` / 分隔符）；
//!   2. 单份 `SKILL.md` ≤ 128 KiB（防止把系统提示词预算一次撑爆）；
//!   3. 删除只删 `skills/<key>` 这一层，且解析后必须仍在 skills 根内（挡符号链接逃逸）。
//!
//! 写盘用临时文件 + rename（与 `mcp_config.rs` / `memo.rs` 同口径），写坏不会毁掉已有技能。
//!
//! 「从 URL 安装」（v1 的 raw URL 导入）：只收 `https://`，下载时就地卡 128 KiB
//! （不等收完再判大小），同名技能要用户二次确认才覆盖。失败文案只说「哪一步没成」，
//! 状态码 / 底层错误只进 stderr（docs/forbidden.md §59）。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// 单份 `SKILL.md` 的上限：清单预算只有 8 000 字符，正文也远用不到 128 KiB。
const MAX_SKILL_BYTES: usize = 128 * 1024;

/// `key` 的最长字符数（文件夹名）。
const MAX_KEY_CHARS: usize = 64;

/// 下载地址的长度上限（挡住把整段 base64 塞进 URL 的花招）。
const MAX_URL_CHARS: usize = 2048;

/// 下载超时：技能正文是纯文本，给足但不无限等。
const FETCH_TIMEOUT: Duration = Duration::from_secs(20);

/// 与 v1 导入同款的 UA —— 不少 raw 托管会对空 UA 直接 403。
const UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
                  (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36";

/// 进程级复用（与 `translate.rs` 同一范式）。
static HTTP_CLIENT: OnceLock<Result<reqwest::blocking::Client, String>> = OnceLock::new();

fn http() -> Result<reqwest::blocking::Client, String> {
    HTTP_CLIENT
        .get_or_init(|| {
            reqwest::blocking::Client::builder()
                .timeout(FETCH_TIMEOUT)
                .build()
                .map_err(|e| {
                    eprintln!("[lunac-skills] 初始化 HTTP 客户端失败: {e}");
                    "网络组件初始化失败，请稍后再试".to_string()
                })
        })
        .clone()
}

/// 技能目录：`<exe 根>/skills`。
pub fn skills_dir(root: &Path) -> PathBuf {
    root.join("skills")
}

/// 列出全部技能：`{ "entries": [...], "error": "…" }`。
///
/// `entries[]`：`key` / `name` / `description` / `fork` / `bytes` / `valid`。
/// 坏文件不隐藏（`valid=false`），排其它条目前面读到的第一个错误进 `error`。
pub fn list_json(root: &Path) -> String {
    let dir = skills_dir(root);
    if !dir.exists() {
        return json!({ "entries": [], "error": "" }).to_string();
    }
    let mut keys: Vec<String> = Vec::new();
    let mut first_error = String::new();
    match std::fs::read_dir(&dir) {
        Ok(items) => {
            for item in items.flatten() {
                let path = item.path();
                if !path.is_dir() {
                    continue;
                }
                let key = item.file_name().to_string_lossy().to_string();
                keys.push(key);
            }
        }
        Err(error) => {
            return json!({
                "entries": [],
                "error": format!("读不了技能目录（{}）：{error}", dir.display()),
            })
            .to_string()
        }
    }
    keys.sort();

    let mut entries = Vec::new();
    for key in keys {
        let path = dir.join(&key).join("SKILL.md");
        match std::fs::read_to_string(&path) {
            Ok(text) => {
                let meta = frontmatter(&text);
                entries.push(json!({
                    "key": key,
                    "name": meta.name,
                    "description": meta.description,
                    "fork": meta.fork,
                    "allowedTools": meta.allowed_tools,
                    // 清单 / 哈希校验要用的指纹：同一条技能在任何机器上都应是这串。
                    "sha256": sha256_hex(text.as_bytes()),
                    "bytes": text.len(),
                    "valid": true,
                }));
            }
            Err(error) => {
                if first_error.is_empty() {
                    first_error = format!("{key}/SKILL.md 读不了：{error}");
                }
                entries.push(json!({
                    "key": key,
                    "name": "",
                    "description": "",
                    "fork": false,
                    "allowedTools": [],
                    "sha256": "",
                    "bytes": 0,
                    "valid": false,
                }));
            }
        }
    }
    json!({ "entries": entries, "error": first_error }).to_string()
}

// ── 「精选」索引（§待办 3-28.9 ①）────────────────────────────────────
//
// 清单随包分发在 `<exe 根>/skills/curated.json`（`scripts/build-release-v2.ps1` 生成：
// 条目里的 `sha256` 由 `agent-templates/skills/<key>/SKILL.md` 现场算出）；正文镜像在
// `<exe 根>/agent-templates/skills/<key>/`。这里只做三件事：
//   · 解析清单（缺 key / 缺真实哈希的条目直接跳过，不做「看起来验证过」的假象）；
//   · 与已装技能逐条比对（installed / outdated / missing）；
//   · 安装**复用** `install_with` —— 哈希 / 危险扫描 / 权限 diff 一道闸都不绕。
//
// 边界如实记：这份清单是「随包 6 个官方技能」的哈希钉住版，不是远端市场；
// 远端条目的接入点就是同一份 schema（`url` + `sha256`），机制已就绪。

const CURATED_FILE: &str = "curated.json";
const MAX_CURATED_ENTRIES: usize = 50;
const CURATED_SCHEMA_VERSION: u64 = 1;

/// 随包精选的正文镜像目录（只读，不随用户编辑变化）。
pub fn bundled_skill_dir(root: &Path) -> PathBuf {
    root.join("agent-templates").join("skills")
}

pub fn curated_manifest_path(root: &Path) -> PathBuf {
    skills_dir(root).join(CURATED_FILE)
}

struct CuratedRow {
    key: String,
    description: String,
    sha256: String,
    files: Vec<String>,
}

fn is_sha256_hex(raw: &str) -> bool {
    raw.len() == 64 && raw.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn load_curated(root: &Path) -> Result<(String, Vec<CuratedRow>), String> {
    let path = curated_manifest_path(root);
    let text = std::fs::read_to_string(&path)
        .map_err(|_| "没有「精选」清单（随包分发的 skills/curated.json）".to_string())?;
    let value: Value = serde_json::from_str(&text).map_err(|error| {
        eprintln!("[lunac-skills] 精选清单解析失败（{}）: {error}", path.display());
        "精选清单格式不对（随包文件可能已损坏）：请重新安装应用".to_string()
    })?;
    let version = value.get("version").and_then(Value::as_u64).unwrap_or(0);
    if version != CURATED_SCHEMA_VERSION {
        return Err(format!(
            "精选清单版本不支持：{version}（本版只认 {CURATED_SCHEMA_VERSION}）"
        ));
    }
    let updated = value
        .get("updated")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut rows: Vec<CuratedRow> = Vec::new();
    let items = value
        .get("entries")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for item in items {
        if rows.len() >= MAX_CURATED_ENTRIES {
            break;
        }
        let key = item
            .get("key")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        let sha256 = item
            .get("sha256")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        // 没钉哈希的条目不可验证 ⇒ 不进列表（沉默跳过，由 `error` 之外的地方核对发布脚本）。
        if check_key(&key).is_err() || !is_sha256_hex(&sha256) {
            continue;
        }
        if rows.iter().any(|row| row.key == key) {
            continue;
        }
        rows.push(CuratedRow {
            key,
            description: item
                .get("description")
                .and_then(Value::as_str)
                .unwrap_or("")
                .trim()
                .to_string(),
            sha256,
            files: item
                .get("files")
                .and_then(Value::as_array)
                .map(|items| {
                    items
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_string)
                        .collect()
                })
                .unwrap_or_default(),
        });
    }
    Ok((updated, rows))
}

/// 「精选」清单 + 与已装技能的比对：`{ok, version, updated, entries[], error}`。
///
/// 每条 `status`：`installed`（哈希一致）/ `outdated`（已装但内容不同）/ `missing`（没装）。
pub fn curated_json(root: &Path) -> String {
    match load_curated(root) {
        Ok((updated, rows)) => {
            let entries: Vec<Value> = rows
                .iter()
                .map(|row| {
                    let installed =
                        std::fs::read_to_string(skills_dir(root).join(&row.key).join("SKILL.md"))
                            .ok();
                    let installed_sha = installed
                        .as_ref()
                        .map(|text| sha256_hex(text.as_bytes()))
                        .unwrap_or_default();
                    let status = if installed.is_none() {
                        "missing"
                    } else if installed_sha == row.sha256 {
                        "installed"
                    } else {
                        "outdated"
                    };
                    json!({
                        "key": row.key,
                        "description": row.description,
                        "sha256": row.sha256,
                        "installedSha256": installed_sha,
                        "files": row.files,
                        "status": status,
                    })
                })
                .collect();
            json!({
                "ok": true,
                "version": CURATED_SCHEMA_VERSION,
                "updated": updated,
                "entries": entries,
                "error": "",
            })
            .to_string()
        }
        Err(reason) => json!({
            "ok": false,
            "version": CURATED_SCHEMA_VERSION,
            "updated": "",
            "entries": [],
            "error": reason,
        })
        .to_string(),
    }
}

/// 从「精选」安装 / 修复：入参 `{ key, overwrite, acceptScan, acceptPermissions }`。
///
/// 复用 `install_with`：`sha256` 钉死成清单值，来源换成随包镜像（不联网）。
pub fn install_curated_json(root: &Path, request: &Value) -> String {
    let key = request
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Err(reason) = check_key(&key) {
        return install_error(&key, false, &reason);
    }
    let row = match load_curated(root) {
        Ok((_, rows)) => match rows.into_iter().find(|row| row.key == key) {
            Some(row) => row,
            None => {
                return install_error(&key, false, &format!("「精选」清单里没有 {key}"));
            }
        },
        Err(reason) => return install_error(&key, false, &reason),
    };
    let mut merged = request.as_object().cloned().unwrap_or_default();
    merged.insert("key".to_string(), json!(key.clone()));
    merged.insert("url".to_string(), json!(format!("bundled:{key}")));
    merged.insert("bundled".to_string(), json!(true));
    merged.insert("sha256".to_string(), json!(row.sha256));
    install_with(
        root,
        &Value::Object(merged),
        &|_| Err("随包精选安装不联网".to_string()),
    )
}

/// 读一份技能的原始文本：`{ ok, key, content, error }`。
pub fn read_json(root: &Path, key: &str) -> String {
    let key = key.trim();
    if let Err(reason) = check_key(key) {
        return json!({ "ok": false, "key": key, "content": "", "error": reason }).to_string();
    }
    let path = skills_dir(root).join(key).join("SKILL.md");
    match std::fs::read_to_string(&path) {
        Ok(content) => {
            let meta = frontmatter(&content);
            json!({
                "ok": true,
                "key": key,
                "content": content,
                "sha256": sha256_hex(content.as_bytes()),
                "allowedTools": meta.allowed_tools,
                "error": "",
            })
            .to_string()
        }
        Err(error) => json!({
            "ok": false,
            "key": key,
            "content": "",
            "sha256": "",
            "allowedTools": [],
            "error": format!("读不了 {key}/SKILL.md：{error}"),
        })
        .to_string(),
    }
}

/// 写入一份技能（新建 / 覆盖同一个 key）：`{ ok, key, error }`。
///
/// 成功后留一条审计行（`save` / 覆盖时 `overwrite=true`）—— 编辑器里改技能同样要能追溯。
pub fn save_json(root: &Path, request: &Value) -> String {
    let key = request
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    let content = request
        .get("content")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let existed = skills_dir(root).join(&key).join("SKILL.md").exists();
    match write_skill_file(root, &key, content) {
        Ok(()) => {
            let scan = crate::skill_scan::scan(content);
            audit(
                root,
                "Save",
                &key,
                "allow",
                "USER_ALLOW",
                if existed {
                    "在编辑器里覆盖了技能（旧正文已被替换）"
                } else {
                    "在编辑器里新建了技能"
                },
                !existed,
                json!({
                    "bytes": content.len(),
                    "risk": scan_risk(&scan),
                    "findings": scan_findings(&scan),
                    "overwrite": existed,
                }),
            );
            json!({ "ok": true, "key": key, "error": "" }).to_string()
        }
        Err(reason) => json!({ "ok": false, "key": key, "error": reason }).to_string(),
    }
}

/// 从 raw URL 安装一份技能：入参 `{ key, url, overwrite }`，回执
/// `{ ok, key, bytes, name, exists, error }`。
///
/// `exists=true` 且 `ok=false` 表示「同名技能已存在，等用户确认覆盖」——
/// 界面据此弹二次确认，再带 `overwrite=true` 重发一次。
pub fn install_json(root: &Path, request: &Value) -> String {
    match http() {
        Ok(client) => install_with(root, request, &|url| fetch_text(&client, url)),
        Err(reason) => install_error("", false, &reason),
    }
}

/// 安装的可测内核：`fetch` 由调用方注入，测试不碰网络。
fn install_with(root: &Path, request: &Value, fetch: &dyn Fn(&str) -> Result<String, String>) -> String {
    let key = request
        .get("key")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    if let Err(reason) = check_key(&key) {
        return install_error(&key, false, &reason);
    }
    let url = request
        .get("url")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_string();
    // 随包精选（§待办 3-28.9 ①）：内容是安装包里那份只读镜像，不联网 ⇒ 不校验 URL；
    // 其余来源仍然只收 https。两种来源的哈希 / 扫描 / 权限 diff 闸完全同一条路。
    let bundled = request
        .get("bundled")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if !bundled {
        if let Err(reason) = check_url(&url) {
            return install_error(&key, false, &reason);
        }
    }
    let overwrite = request
        .get("overwrite")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let existed = skills_dir(root).join(&key).join("SKILL.md").exists();
    if existed && !overwrite {
        return install_error(&key, true, &format!("技能 {key} 已存在：确认覆盖后再导入"));
    }
    let content = if bundled {
        let mirror = bundled_skill_dir(root).join(&key).join("SKILL.md");
        match std::fs::read_to_string(&mirror) {
            Ok(text) => text,
            Err(error) => {
                // 原始错误只进 stderr（forbidden.md §禁用 6）：界面上只说「哪一步没成」。
                eprintln!("[lunac-skills] 读随包精选镜像失败（{}）: {error}", mirror.display());
                return install_error(
                    &key,
                    false,
                    &format!(
                        "随包精选里没有 {key} 的正文（{} 读不了）：请重新安装应用后再试",
                        mirror.display()
                    ),
                );
            }
        }
    } else {
        match fetch(&url) {
            Ok(text) => text,
            Err(reason) => return install_error(&key, false, &reason),
        }
    };
    // ② 哈希校验（§待办 3-28.9）：清单或用户钉住的 sha256 必须是这一份内容。
    // 大小写不敏感；不填 = 不校验（本地手输 URL 的老路径行为不变）。
    let digest = sha256_hex(content.as_bytes());
    let expected = request
        .get("sha256")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !expected.is_empty() && expected != digest {
        audit(
            root,
            "Install",
            &key,
            "deny",
            "HASH_MISMATCH",
            "安装被哈希校验拦下（下载到的内容与清单不符）",
            false,
            json!({
                "bytes": content.len(),
                "sha256": digest,
                "expected": expected,
                "url": url,
            }),
        );
        return json!({
            "ok": false,
            "key": key,
            "bytes": 0,
            "name": "",
            "exists": false,
            "scanBlocked": false,
            "hashMismatch": true,
            "permissionBlocked": false,
            "sha256": digest,
            "permissions": Value::Null,
            "error": format!(
                "内容哈希不符（清单 {}…，下载到 {}…）：可能下错了文件或被改动，已拒绝安装",
                short_hash(&expected),
                short_hash(&digest),
            ),
        })
        .to_string();
    }
    // 安装前安全闸（docs/todo.md §待办 3-28.9）：命中 danger 必须先让用户看检查结果。
    // 判定本身只在 `skill_scan` 里（与界面上的「检查」按钮同一份实现）。
    let scan = crate::skill_scan::scan(&content);
    let risk = scan_risk(&scan).to_string();
    let danger = risk == "danger";
    let accept_scan = request
        .get("acceptScan")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    if danger && !accept_scan {
        // 拒装也是判定，要留痕（§待办 3-28.9 ④）：审计里能看出「有过这次尝试」。
        audit(
            root,
            "Install",
            &key,
            "deny",
            "DANGER_RULE",
            "从 URL 安装被安全扫描拦下（危险命令，等用户确认）",
            false,
            json!({
                "bytes": content.len(),
                "risk": risk,
                "findings": scan_findings(&scan),
                "url": url,
                "sha256": digest,
            }),
        );
        return json!({
            "ok": false,
            "key": key,
            "bytes": 0,
            "name": "",
            "exists": false,
            "scanBlocked": true,
            "hashMismatch": false,
            "permissionBlocked": false,
            "sha256": digest,
            "permissions": Value::Null,
            "scan": scan,
            "error": "这个技能里有危险命令（整盘删除 / 远程执行 / 凭据外传）：先看检查结果，确认后再安装",
        })
        .to_string();
    }
    // ③ 权限 diff（§待办 3-28.9）：覆盖安装时，`allowed-tools` 变大（含「从有限名单
    // 变成不限制」）必须重新确认 —— 否则一次升级就能悄悄多拿工具。
    let meta = frontmatter(&content);
    let previous_tools = std::fs::read_to_string(skills_dir(root).join(&key).join("SKILL.md"))
        .map(|text| frontmatter(&text).allowed_tools)
        .unwrap_or_default();
    let incoming: BTreeSet<String> = meta.allowed_tools.iter().cloned().collect();
    let previous: BTreeSet<String> = previous_tools.iter().cloned().collect();
    let added: Vec<String> = incoming.difference(&previous).cloned().collect();
    let removed: Vec<String> = previous.difference(&incoming).cloned().collect();
    // 「扩大权限」= 原来有限名单，现在要么新增了工具，要么直接变成不限制。
    // 反过来（原来不限制、现在收窄）不需要确认。
    let unrestricted = meta.allowed_tools.is_empty();
    let broadens = existed
        && if previous.is_empty() {
            false
        } else if unrestricted {
            true
        } else {
            !added.is_empty()
        };
    let accept_permissions = request
        .get("acceptPermissions")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let permissions = json!({
        "tools": meta.allowed_tools.clone(),
        "unrestricted": unrestricted,
        "added": added,
        "removed": removed,
        "broadens": broadens,
    });
    if broadens && !accept_permissions {
        audit(
            root,
            "Install",
            &key,
            "deny",
            "PERMISSION_EXPAND",
            "覆盖安装被权限 diff 拦下（allowed-tools 变大，等用户确认）",
            false,
            json!({
                "bytes": content.len(),
                "sha256": digest,
                "permissionsAdded": added,
                "url": url,
            }),
        );
        return json!({
            "ok": false,
            "key": key,
            "bytes": 0,
            "name": "",
            "exists": false,
            "scanBlocked": false,
            "hashMismatch": false,
            "permissionBlocked": true,
            "sha256": digest,
            "permissions": permissions,
            "scan": scan,
            "error": format!(
                "这次升级扩大了工具权限（{}）：确认后才安装",
                permission_change_text(&added, unrestricted, &previous_tools),
            ),
        })
        .to_string();
    }
    let bytes = content.len();
    if let Err(reason) = write_skill_file(root, &key, &content) {
        return install_error(&key, false, &reason);
    }
    audit(
        root,
        "Install",
        &key,
        "allow",
        "USER_ALLOW",
        if existed {
            if bundled {
                "从随包精选覆盖安装了技能"
            } else {
                "从 URL 覆盖安装技能"
            }
        } else if bundled {
            "从随包精选安装了技能"
        } else {
            "从 URL 安装了技能"
        },
        !existed,
        json!({
            "bytes": bytes,
            "risk": risk,
            "findings": scan_findings(&scan),
            "url": url,
            "overwrite": existed,
            "sha256": digest,
            "permissionsAdded": permissions["added"],
            "permissionsBroadened": broadens,
            // 用户看过危险扫描结果仍选择安装 —— 事后复盘的第一手证据。
            "scanAccepted": danger && accept_scan,
        }),
    );
    json!({
        "ok": true,
        "key": key,
        "bytes": bytes,
        "name": meta.name,
        "exists": false,
        "scanBlocked": false,
        "hashMismatch": false,
        "permissionBlocked": false,
        "sha256": digest,
        "permissions": permissions,
        "scan": scan,
        "error": "",
    })
    .to_string()
}

/// 哈希前缀（界面上只显示 12 位，避免整串刷屏）。
fn short_hash(hash: &str) -> String {
    hash.chars().take(12).collect()
}

/// `SKILL.md` 内容的 SHA-256（小写十六进制）。
fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// 权限扩大的那句人话：是「变成不限制」还是「新增了哪几个工具」。
fn permission_change_text(added: &[String], unrestricted: bool, previous: &[String]) -> String {
    if unrestricted {
        return if previous.is_empty() {
            "任意工具".to_string()
        } else {
            "从有限名单变成不限制（任意工具）".to_string()
        };
    }
    added.join("、")
}

fn install_error(key: &str, exists: bool, reason: &str) -> String {
    json!({
        "ok": false,
        "key": key,
        "bytes": 0,
        "name": "",
        "exists": exists,
        "scanBlocked": false,
        "hashMismatch": false,
        "permissionBlocked": false,
        "sha256": "",
        "permissions": Value::Null,
        "error": reason,
    })
    .to_string()
}

/// 下载地址只收 `https://`（明文 HTTP 会被中间人塞任意提示词）。
fn check_url(url: &str) -> Result<(), String> {
    if url.is_empty() {
        return Err("导入地址不能为空".to_string());
    }
    if url.chars().count() > MAX_URL_CHARS {
        return Err(format!("导入地址太长（上限 {MAX_URL_CHARS} 字符）"));
    }
    let lower = url.to_ascii_lowercase();
    if !lower.starts_with("https://") {
        return Err("只支持 https:// 地址（明文 http 不安全，请改用 https 的 raw 链接）".to_string());
    }
    Ok(())
}

/// 拉一份纯文本，**边收边卡**上限：超了就断开，不等收完。
fn fetch_text(client: &reqwest::blocking::Client, url: &str) -> Result<String, String> {
    let mut response = client
        .get(url)
        .header("User-Agent", UA)
        .send()
        .map_err(|e| {
            eprintln!("[lunac-skills] 下载失败 {url}: {e}");
            "下载失败：连不上这个地址，请确认网络与链接".to_string()
        })?;
    let status = response.status();
    if !status.is_success() {
        eprintln!("[lunac-skills] 下载返回 {status}: {url}");
        return Err(format!(
            "下载失败（服务器返回 {status}）：请确认这个链接能匿名访问"
        ));
    }
    // 跟随重定向后可能降级到 http —— 只认最终的 https。
    if response.url().scheme() != "https" {
        eprintln!("[lunac-skills] 重定向到非 https：{}", response.url());
        return Err("这个链接被重定向到了非 https 地址，已拒绝导入".to_string());
    }
    if let Some(length) = response.content_length() {
        if length as usize > MAX_SKILL_BYTES {
            return Err(oversize_message(length as usize));
        }
    }
    read_capped(&mut response)
}

/// 边读边卡上限：超过 128 KiB 立刻停，不等把整份内容读完。
fn read_capped(reader: &mut dyn std::io::Read) -> Result<String, String> {
    let mut buffer: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => {
                if buffer.len() + read > MAX_SKILL_BYTES {
                    return Err(oversize_message(buffer.len() + read));
                }
                buffer.extend_from_slice(&chunk[..read]);
            }
            Err(e) => {
                eprintln!("[lunac-skills] 下载中断: {e}");
                return Err("下载中断：网络连接中途断开，请重试".to_string());
            }
        }
    }
    String::from_utf8(buffer)
        .map_err(|_| "下载到的内容不是 UTF-8 文本（技能必须是 UTF-8 的 SKILL.md）".to_string())
}

fn oversize_message(bytes: usize) -> String {
    format!(
        "内容太大（{bytes} 字节）：技能正文上限 {} KiB；把长资料放进技能目录的其它文件",
        MAX_SKILL_BYTES / 1024
    )
}

/// 实际落盘：key / 内容三道校验 + 临时文件 + rename。
///
/// 只回 `Ok(())` / `Err(一句人话)`；**不**负责审计与回执（安装 / 编辑两条路各自记）。
fn write_skill_file(root: &Path, key: &str, content: &str) -> Result<(), String> {
    check_key(key)?;
    if content.trim().is_empty() {
        return Err("内容为空：至少要有一段 SKILL.md 正文".to_string());
    }
    if content.len() > MAX_SKILL_BYTES {
        return Err(oversize_message(content.len()));
    }
    let dir = skills_dir(root).join(key);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("建不了技能目录（{}）：{error}", dir.display()))?;
    let path = dir.join("SKILL.md");
    let tmp = dir.join(".SKILL.md.tmp");
    std::fs::write(&tmp, content)
        .map_err(|error| format!("写不进 {}：{error}；请确认目录可写", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .map_err(|error| format!("替换 {} 失败：{error}", path.display()))?;
    Ok(())
}

/// 扫描结论里的分级（`safe` / `warn` / `danger`），缺省当 `safe`。
fn scan_risk(scan: &Value) -> &str {
    scan.get("risk").and_then(Value::as_str).unwrap_or("safe")
}

/// 扫描结论里的命中条数（审计里只记条数，逐条正文在安装回执 / 编辑器里看）。
fn scan_findings(scan: &Value) -> usize {
    scan.get("findings")
        .and_then(Value::as_array)
        .map(Vec::len)
        .unwrap_or(0)
}

/// 写一条技能审计行（`docs/todo.md` §待办 3-28.9 ④：安装 / 升级 / 禁用都要留痕）。
///
/// 复用白箱边界账本（`security_ledger`）—— 「谁在什么时候装 / 改 / 删了哪个技能」与
/// 审批判定同一本账、同一套脱敏与 14 天轮转，**不新开第二本**（`docs/forbidden.md` §禁用 62）。
/// `actor` 固定 `user`：这些动作只可能来自设置面板里用户点的那一下。
///
/// 只记**状态变化**（成功落盘 / 被安全闸拦下）；写盘失败不记账，否则审计里全是噪声。
/// 记账失败只进 stderr —— 审计是旁路，不能因为它写不进去就拒绝用户的正常操作。
#[allow(clippy::too_many_arguments)]
fn audit(
    root: &Path,
    action: &str,
    key: &str,
    decision: &str,
    code: &str,
    reason_text: &str,
    reversible: bool,
    evidence: Value,
) {
    // 时间与分片键都取宿主 UTC（与账本同口径；本模块没有前端日期可传）。
    let (_, date) = crate::security_ledger::utc_now();
    let entry = json!({
        "actor": "user",
        "tool": format!("Skills.{action}"),
        "target": key,
        "decision": decision,
        "reasonCode": code,
        "reasonText": reason_text,
        "policySource": format!("skills_admin.rs#{}", action.to_ascii_lowercase()),
        "scope": format!("skill:{key}"),
        "evidence": evidence,
        "reversible": reversible,
    });
    if let Err(reason) = crate::security_ledger::append(root, &date, &entry) {
        eprintln!("[lunac-skills] 审计行写不进去（{action} {key}）：{reason}");
    }
}

/// 删除一份技能（整个 `skills/<key>` 目录）：`{ ok, key, error }`；不存在也算成功。
pub fn delete_json(root: &Path, key: &str) -> String {
    let key = key.trim();
    if let Err(reason) = check_key(key) {
        return json!({ "ok": false, "key": key, "error": reason }).to_string();
    }
    let dir = skills_dir(root);
    let target = dir.join(key);
    if !target.exists() {
        return json!({ "ok": true, "key": key, "error": "" }).to_string();
    }
    // 符号链接：解析后必须仍在 skills 根内（否则 `remove_dir_all` 会删到外面去）。
    let Ok(root_real) = dir.canonicalize() else {
        return json!({ "ok": false, "key": key, "error": "技能目录不可用" }).to_string();
    };
    let Ok(target_real) = target.canonicalize() else {
        return json!({ "ok": false, "key": key, "error": "技能目录不可用" }).to_string();
    };
    if target_real == root_real || !target_real.starts_with(&root_real) {
        return json!({ "ok": false, "key": key, "error": "路径越出技能目录，已拒绝" }).to_string();
    }
    match std::fs::remove_dir_all(&target_real) {
        Ok(()) => {
            // 「禁用 / 卸载」的审计行（§待办 3-28.9 ④）：删掉的内容没法回滚，如实标 irreversible。
            audit(
                root,
                "Delete",
                key,
                "allow",
                "USER_ALLOW",
                "删除了技能（整个目录，含自带脚本与资源）",
                false,
                json!({}),
            );
            json!({ "ok": true, "key": key, "error": "" }).to_string()
        }
        Err(error) => json!({
            "ok": false,
            "key": key,
            "error": format!("删不掉 {key}：{error}；请确认文件没被占用"),
        })
        .to_string(),
    }
}

/// `SKILL.md` frontmatter 里界面 / 安装闸要用的几项。
///
/// `allowed_tools` 与 agent（`core-agent/src/skills.rs` 的 `parse_frontmatter`）**同口径**：
/// `[a, b]` / `a, b` / `a b` 三种写法都认，空 = 不限制。
#[derive(Default)]
struct Meta {
    name: String,
    description: String,
    fork: bool,
    allowed_tools: Vec<String>,
}

/// 从 frontmatter 抽界面要的几项；没有 frontmatter 就回空值。
///
/// 只认单行 `key: value`（与 core-agent 的最小解析同口径），`context: fork` 才算子代理技能。
fn frontmatter(content: &str) -> Meta {
    let mut meta = Meta::default();
    let body = content.strip_prefix("---").unwrap_or("");
    if body.is_empty() {
        return meta;
    }
    // 开头可能是 `---\r\n` 或 `---\n`。
    let body = body.strip_prefix('\r').unwrap_or(body);
    let body = body.strip_prefix('\n').unwrap_or(body);
    let mut lines = body.lines();
    for line in lines.by_ref() {
        let trimmed = line.trim();
        if trimmed == "---" {
            break;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            continue;
        };
        let value = value.trim().trim_matches('"').trim_matches('\'').to_string();
        match key.trim() {
            "name" if meta.name.is_empty() => meta.name = value,
            "description" if meta.description.is_empty() => meta.description = value,
            "context" if value.eq_ignore_ascii_case("fork") => meta.fork = true,
            "allowed-tools" if meta.allowed_tools.is_empty() => {
                meta.allowed_tools = value
                    .trim_start_matches('[')
                    .trim_end_matches(']')
                    .split([',', ' ', '\t'])
                    .map(|s| s.trim().trim_matches('"').trim_matches('\''))
                    .filter(|s| !s.is_empty())
                    .map(str::to_string)
                    .collect();
            }
            _ => {}
        }
    }
    meta
}

/// `key`（文件夹名）只允许 `[A-Za-z0-9_-]`，≤ 64 字符。
fn check_key(key: &str) -> Result<(), String> {
    if key.is_empty() {
        return Err("技能 key 不能为空".to_string());
    }
    if key.chars().count() > MAX_KEY_CHARS {
        return Err(format!("技能 key 太长（上限 {MAX_KEY_CHARS} 字符）"));
    }
    if key == "." || key == ".." {
        return Err("技能 key 不合法".to_string());
    }
    if !key
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
    {
        return Err(format!(
            "技能 key 只能包含字母 / 数字 / `_` / `-`（收到 {key}）"
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "lunac-skills-admin-{tag}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp root");
        dir
    }

    fn save(root: &Path, key: &str, content: &str) -> Value {
        let raw = save_json(root, &json!({ "key": key, "content": content }));
        serde_json::from_str(&raw).expect("valid json")
    }

    fn entries(root: &Path) -> Vec<Value> {
        let raw: Value = serde_json::from_str(&list_json(root)).expect("valid json");
        raw.get("entries")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn read_value(root: &Path, key: &str) -> Value {
        serde_json::from_str(&read_json(root, key)).expect("valid json")
    }

    /// §待办 3-28.9 ①：「精选」清单只收钉了真实哈希的条目；状态机
    /// missing → installed → outdated → installed；安装复用同一条闸。
    #[test]
    fn curated_index_pins_hashes_and_installs_from_the_bundled_mirror() {
        let root = temp_root("curated");
        let body = "---\nname: demo\ndescription: 演示技能\nallowed-tools: Read\n---\n正文\n";
        let hash = sha256_hex(body.as_bytes());
        let mirror = bundled_skill_dir(&root).join("demo");
        std::fs::create_dir_all(&mirror).unwrap();
        std::fs::create_dir_all(skills_dir(&root)).unwrap();
        std::fs::write(mirror.join("SKILL.md"), body).unwrap();
        std::fs::write(
            curated_manifest_path(&root),
            json!({
                "version": 1,
                "updated": "2026-11-19",
                "entries": [
                    {"key": "demo", "description": "演示技能", "sha256": hash},
                    {"key": "unsigned", "description": "缺哈希", "sha256": ""},
                    {"key": "../escape", "description": "非法 key", "sha256": hash},
                ],
            })
            .to_string(),
        )
        .unwrap();

        let listed: Value = serde_json::from_str(&curated_json(&root)).unwrap();
        assert_eq!(listed["ok"], true);
        let items = listed["entries"].as_array().unwrap();
        assert_eq!(items.len(), 1, "缺哈希 / 非法 key 的条目必须被忽略");
        assert_eq!(items[0]["status"], "missing");

        // 从镜像安装：哈希与清单一致才落盘，内容逐字节等于镜像。
        let installed: Value =
            serde_json::from_str(&install_curated_json(&root, &json!({"key": "demo"}))).unwrap();
        assert_eq!(installed["ok"], true, "{}", installed["error"]);
        assert_eq!(
            std::fs::read_to_string(skills_dir(&root).join("demo").join("SKILL.md")).unwrap(),
            body
        );
        let listed: Value = serde_json::from_str(&curated_json(&root)).unwrap();
        assert_eq!(listed["entries"][0]["status"], "installed");

        // 本地改过 ⇒ outdated；覆盖安装后恢复成精选版本。
        save(&root, "demo", "---\nname: demo\ndescription: 本地改过\n---\n别的正文\n");
        let listed: Value = serde_json::from_str(&curated_json(&root)).unwrap();
        assert_eq!(listed["entries"][0]["status"], "outdated");
        let restored: Value = serde_json::from_str(&install_curated_json(
            &root,
            &json!({"key": "demo", "overwrite": true}),
        ))
        .unwrap();
        assert_eq!(restored["ok"], true, "{}", restored["error"]);
        let listed: Value = serde_json::from_str(&curated_json(&root)).unwrap();
        assert_eq!(listed["entries"][0]["status"], "installed");

        // 清单里没有的 key / 清单缺失都如实拒绝。
        let unknown: Value =
            serde_json::from_str(&install_curated_json(&root, &json!({"key": "nope"}))).unwrap();
        assert_eq!(unknown["ok"], false);
        std::fs::remove_file(curated_manifest_path(&root)).unwrap();
        let missing: Value = serde_json::from_str(&curated_json(&root)).unwrap();
        assert_eq!(missing["ok"], false);
        assert!(missing["error"].as_str().unwrap().contains("精选"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn list_read_save_delete_round_trip() {
        let root = temp_root("roundtrip");
        assert!(entries(&root).is_empty(), "目录不存在 ⇒ 空表且不 panic");

        let saved = save(
            &root,
            "code-review",
            "---\nname: 代码审查\ndescription: 审一遍改动\n---\n正文",
        );
        assert_eq!(saved["ok"], true);
        let list = entries(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["key"], "code-review");
        assert_eq!(list[0]["name"], "代码审查");
        assert_eq!(list[0]["description"], "审一遍改动");
        assert_eq!(list[0]["fork"], false);
        assert_eq!(list[0]["valid"], true);

        let raw: Value =
            serde_json::from_str(&read_json(&root, "code-review")).expect("valid json");
        assert_eq!(raw["ok"], true);
        assert!(raw["content"].as_str().unwrap().contains("正文"));

        let removed: Value =
            serde_json::from_str(&delete_json(&root, "code-review")).expect("valid json");
        assert_eq!(removed["ok"], true);
        assert!(entries(&root).is_empty());
        // 幂等：再删一次也成功。
        let again: Value =
            serde_json::from_str(&delete_json(&root, "code-review")).expect("valid json");
        assert_eq!(again["ok"], true);
    }

    /// `context: fork` 才算子代理技能（与 core-agent 的解析同口径）。
    #[test]
    fn fork_flag_comes_from_context_fork() {
        let root = temp_root("fork");
        save(
            &root,
            "forked",
            "---\ndescription: 干活\ncontext: fork\n---\nX",
        );
        save(&root, "plain", "---\ndescription: 读\n---\nY");
        let list = entries(&root);
        assert_eq!(list[0]["key"], "forked");
        assert_eq!(list[0]["fork"], true);
        assert_eq!(list[1]["fork"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 三道闸：key 非法 / 内容为空 / 内容超上限都拒绝，且不落盘。
    #[test]
    fn invalid_key_and_oversize_content_are_rejected() {
        let root = temp_root("guards");
        for key in ["", "..", "a/b", "a\\b", "有中文", &"k".repeat(65)] {
            let result = save(&root, key, "---\ndescription: x\n---\nX");
            assert_eq!(result["ok"], false, "必须拒绝 key={key:?}");
        }
        let empty = save(&root, "ok", "   \n\t");
        assert_eq!(empty["ok"], false);
        let huge = "x".repeat(MAX_SKILL_BYTES + 1);
        let oversize = save(&root, "ok", &huge);
        assert_eq!(oversize["ok"], false);
        assert!(oversize["error"].as_str().unwrap().contains("KiB"));
        assert!(entries(&root).is_empty(), "被拒绝的写入不能留文件");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 没有 frontmatter 也算有效（正文可以就是纯 markdown）。
    #[test]
    fn plain_markdown_without_frontmatter_is_valid() {
        let root = temp_root("plain");
        save(&root, "plain", "只有正文");
        let list = entries(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["valid"], true);
        assert_eq!(list[0]["name"], "");
        assert_eq!(list[0]["description"], "");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 缺 `SKILL.md` 的目录仍然列出来（`valid=false`），不隐藏问题。
    #[test]
    fn directory_without_skill_md_is_listed_as_invalid() {
        let root = temp_root("broken");
        std::fs::create_dir_all(skills_dir(&root).join("empty")).unwrap();
        let list = entries(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["key"], "empty");
        assert_eq!(list[0]["valid"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn install(root: &Path, request: Value, fetch: &dyn Fn(&str) -> Result<String, String>) -> Value {
        serde_json::from_str(&install_with(root, &request, fetch)).expect("valid json")
    }

    /// 正常路径：https 拉下来 → 落盘 → 列表里能看到，`bytes` / `name` 如实回执。
    #[test]
    fn install_from_https_url_writes_the_skill() {
        let root = temp_root("install-ok");
        let content = "---\nname: 网页审查\ndescription: 审一遍网页\n---\n正文";
        let raw = install(
            &root,
            json!({
                "key": "web-review",
                "url": "https://raw.example.com/skills/web-review/SKILL.md",
            }),
            &|url| {
                assert_eq!(url, "https://raw.example.com/skills/web-review/SKILL.md");
                Ok(content.to_string())
            },
        );
        assert_eq!(raw["ok"], true);
        assert_eq!(raw["key"], "web-review");
        assert_eq!(raw["name"], "网页审查");
        assert_eq!(raw["bytes"], json!(content.len()));
        assert_eq!(raw["exists"], false);
        let list = entries(&root);
        assert_eq!(list.len(), 1);
        assert_eq!(list[0]["description"], "审一遍网页");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 只收 https：http / ftp / 空 / 超长 / 非法 key 都在**联网前**拒绝。
    #[test]
    fn install_rejects_unsafe_urls_and_keys_before_fetching() {
        let root = temp_root("install-url");
        let fetched = std::cell::Cell::new(0);
        let fetch = |_: &str| {
            fetched.set(fetched.get() + 1);
            Ok("---\ndescription: x\n---\nX".to_string())
        };
        let empty = install(&root, json!({ "key": "ok", "url": "  " }), &fetch);
        assert_eq!(empty["ok"], false);
        assert!(empty["error"].as_str().unwrap().contains("不能为空"));
        for url in [
            "http://raw.example.com/SKILL.md",
            "ftp://example.com/SKILL.md",
            "file:///C:/SKILL.md",
            &format!("https://x.example.com/{}", "a".repeat(MAX_URL_CHARS)),
        ] {
            let result = install(&root, json!({ "key": "ok", "url": url }), &fetch);
            assert_eq!(result["ok"], false, "必须拒绝 url={url:?}");
        }
        let plain_http = install(
            &root,
            json!({ "key": "ok", "url": "http://raw.example.com/SKILL.md" }),
            &fetch,
        );
        assert!(
            plain_http["error"].as_str().unwrap().contains("https"),
            "拒绝理由要说明只支持 https：{}",
            plain_http["error"]
        );
        let bad_key = install(
            &root,
            json!({ "key": "a/b", "url": "https://raw.example.com/x.md" }),
            &fetch,
        );
        assert_eq!(bad_key["ok"], false);
        assert_eq!(fetched.get(), 0, "校验失败前不能发起下载");
        assert!(entries(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 同名技能要二次确认：第一次回 `exists=true` 且不动原文件，确认后才覆盖。
    #[test]
    fn install_refuses_to_overwrite_without_confirmation() {
        let root = temp_root("install-overwrite");
        save(&root, "dup", "---\ndescription: 旧\n---\n旧正文");
        let fetch = |_: &str| Ok("---\ndescription: 新\n---\n新正文".to_string());

        let refused = install(
            &root,
            json!({ "key": "dup", "url": "https://raw.example.com/dup.md" }),
            &fetch,
        );
        assert_eq!(refused["ok"], false);
        assert_eq!(refused["exists"], true);
        let before: Value =
            serde_json::from_str(&read_json(&root, "dup")).expect("valid json");
        assert!(before["content"].as_str().unwrap().contains("旧正文"));

        let overwritten = install(
            &root,
            json!({
                "key": "dup",
                "url": "https://raw.example.com/dup.md",
                "overwrite": true,
            }),
            &fetch,
        );
        assert_eq!(overwritten["ok"], true);
        let after: Value = serde_json::from_str(&read_json(&root, "dup")).expect("valid json");
        assert!(after["content"].as_str().unwrap().contains("新正文"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 落盘前的大小闸：注入一个超限的下载结果也要被 write_skill 挡住。
    #[test]
    fn install_rejects_oversize_downloads() {
        let root = temp_root("install-oversize");
        let huge = "x".repeat(MAX_SKILL_BYTES + 1);
        let result = install(
            &root,
            json!({ "key": "big", "url": "https://raw.example.com/big.md" }),
            &|_| Ok(huge.clone()),
        );
        assert_eq!(result["ok"], false);
        assert!(result["error"].as_str().unwrap().contains("KiB"));
        assert!(entries(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 安装前的安全闸（§待办 3-28.9）：danger 先回 `scanBlocked`，用户确认后才落盘；
    /// 干净技能直接装，并带回 `scan.risk = "safe"` 让界面能如实显示。
    #[test]
    fn install_blocks_a_dangerous_skill_until_the_scan_is_accepted() {
        let root = temp_root("install-scan");
        let evil = "---\ndescription: 一键清理\n---\nrm -rf /";
        let fetch = |_: &str| Ok(evil.to_string());

        let blocked = install(
            &root,
            json!({ "key": "evil", "url": "https://raw.example.com/evil.md" }),
            &fetch,
        );
        assert_eq!(blocked["ok"], false);
        assert_eq!(blocked["scanBlocked"], true);
        assert_eq!(blocked["scan"]["risk"], "danger");
        assert!(entries(&root).is_empty(), "被拦下的技能不能落盘");

        let accepted = install(
            &root,
            json!({
                "key": "evil",
                "url": "https://raw.example.com/evil.md",
                "acceptScan": true,
            }),
            &fetch,
        );
        assert_eq!(accepted["ok"], true);
        assert_eq!(accepted["scan"]["risk"], "danger", "装上了也要如实标危险");
        assert_eq!(entries(&root).len(), 1);

        let clean = install(
            &root,
            json!({ "key": "clean", "url": "https://raw.example.com/clean.md" }),
            &|_| Ok("---\ndescription: 审一遍\n---\n读 diff 并给结论".to_string()),
        );
        assert_eq!(clean["ok"], true);
        assert_eq!(clean["scan"]["risk"], "safe");
        assert_eq!(clean["scanBlocked"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 清单 / 读取都要带出 `sha256` 与 `allowed-tools`（§待办 3-28.9 ②③ 的数据面）。
    #[test]
    fn list_and_read_expose_sha256_and_allowed_tools() {
        let root = temp_root("meta");
        let content = "---\nname: A\nallowed-tools: [Read, Grep]\n---\n正文";
        save(&root, "a", content);
        let want = sha256_hex(content.as_bytes());
        let list = entries(&root);
        assert_eq!(list[0]["allowedTools"], json!(["Read", "Grep"]));
        assert_eq!(list[0]["sha256"], json!(want));
        let read: Value = serde_json::from_str(&read_json(&root, "a")).expect("valid json");
        assert_eq!(read["sha256"], json!(want));
        assert_eq!(read["allowedTools"], json!(["Read", "Grep"]));
        assert_eq!(read["content"], json!(content));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ② 哈希校验：清单给了 `sha256` 就必须完全一致 —— 不符拒装、留 `HASH_MISMATCH`，
    /// 一致才落盘（大小写不敏感）。
    #[test]
    fn install_rejects_a_hash_mismatch_before_writing() {
        let root = temp_root("install-hash");
        let content = "---\nname: 审查\n---\n正文";
        let good = sha256_hex(content.as_bytes());
        let fetch = |_: &str| Ok(content.to_string());

        let blocked = install(
            &root,
            json!({
                "key": "h",
                "url": "https://raw.example.com/h.md",
                "sha256": "0000000000000000000000000000000000000000000000000000000000000000",
            }),
            &fetch,
        );
        assert_eq!(blocked["ok"], false);
        assert_eq!(blocked["hashMismatch"], true);
        assert_eq!(blocked["sha256"], json!(good), "回执带实际哈希，方便核对清单");
        assert!(entries(&root).is_empty(), "哈希不符不能落盘");
        let rows = ledger(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].decision, "deny");
        assert_eq!(rows[0].reason_code, "HASH_MISMATCH");

        let ok = install(
            &root,
            json!({
                "key": "h",
                "url": "https://raw.example.com/h.md",
                "sha256": good.to_uppercase(),
            }),
            &fetch,
        );
        assert_eq!(ok["ok"], true, "大小写不敏感");
        assert_eq!(ok["sha256"], json!(good));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// ③ 权限 diff：覆盖安装时 `allowed-tools` 变大（新增工具 / 变成不限制）必须确认；
    /// 收窄或不变直接装；首次安装没有可比对象，不弹。
    #[test]
    fn upgrading_a_skill_requires_confirmation_only_when_tools_grow() {
        let root = temp_root("install-perms");
        let base = "---\nname: A\nallowed-tools: Read\n---\n正文";
        assert_eq!(
            install(
                &root,
                json!({ "key": "a", "url": "https://raw.example.com/a.md" }),
                &|_| Ok(base.to_string()),
            )["ok"],
            true,
            "首次安装没有权限 diff"
        );

        // 新增 Write ⇒ 拦下，回执给出 diff，旧正文不动。
        let grown = "---\nname: A\nallowed-tools: Read Write\n---\n正文";
        let blocked = install(
            &root,
            json!({
                "key": "a",
                "url": "https://raw.example.com/a.md",
                "overwrite": true,
            }),
            &|_| Ok(grown.to_string()),
        );
        assert_eq!(blocked["ok"], false);
        assert_eq!(blocked["permissionBlocked"], true);
        assert_eq!(blocked["permissions"]["added"], json!(["Write"]));
        assert_eq!(blocked["permissions"]["broadens"], true);
        assert!(
            read_value(&root, "a")["content"]
                .as_str()
                .unwrap()
                .contains("allowed-tools: Read\n"),
            "被拦下时旧内容必须原样保留"
        );
        assert!(ledger(&root)
            .iter()
            .any(|row| row.decision == "deny" && row.reason_code == "PERMISSION_EXPAND"));

        // 用户确认后才覆盖。
        let accepted = install(
            &root,
            json!({
                "key": "a",
                "url": "https://raw.example.com/a.md",
                "overwrite": true,
                "acceptPermissions": true,
            }),
            &|_| Ok(grown.to_string()),
        );
        assert_eq!(accepted["ok"], true);
        assert!(read_value(&root, "a")["content"]
            .as_str()
            .unwrap()
            .contains("Write"));

        // 收窄（去掉 Write）不需要确认。
        let narrowed = install(
            &root,
            json!({
                "key": "a",
                "url": "https://raw.example.com/a.md",
                "overwrite": true,
            }),
            &|_| Ok(base.to_string()),
        );
        assert_eq!(narrowed["ok"], true);
        assert_eq!(narrowed["permissions"]["removed"], json!(["Write"]));

        // 从有限名单变成不限制（不写 allowed-tools）也必须确认。
        let unrestricted = "---\nname: A\n---\n正文";
        let blocked = install(
            &root,
            json!({
                "key": "a",
                "url": "https://raw.example.com/a.md",
                "overwrite": true,
            }),
            &|_| Ok(unrestricted.to_string()),
        );
        assert_eq!(blocked["permissionBlocked"], true);
        assert_eq!(blocked["permissions"]["unrestricted"], true);
        assert!(
            blocked["error"].as_str().unwrap().contains("不限制"),
            "要把「变成不限制」说清楚：{}",
            blocked["error"]
        );
        let _ = std::fs::remove_dir_all(&root);
    }


    /// 下载流的大小闸：超上限就停，不把整份内容读进内存；非 UTF-8 也拒绝。
    #[test]
    fn read_capped_stops_at_the_budget_and_requires_utf8() {
        use std::io::Cursor;
        let ok = read_capped(&mut Cursor::new("---\ndescription: x\n---\n正文".as_bytes()));
        assert!(ok.expect("utf8").contains("正文"));

        let mut too_big = Cursor::new(vec![b'a'; MAX_SKILL_BYTES + 1]);
        let err = read_capped(&mut too_big).expect_err("超限必须拒绝");
        assert!(err.contains("KiB"));

        let mut invalid = Cursor::new(vec![0xff, 0xfe, 0xfd]);
        let err = read_capped(&mut invalid).expect_err("非 UTF-8 必须拒绝");
        assert!(err.contains("UTF-8"));
    }

    // ── 审计行（§待办 3-28.9 ④）──────────────────────────────────

    fn ledger(root: &Path) -> Vec<crate::security_ledger::Decision> {
        crate::security_ledger::recent(root, 50)
    }

    /// 安装成功要留一条 `allow` / `USER_ALLOW` 的审计行（带 bytes / risk / url）。
    #[test]
    fn a_successful_install_leaves_an_audit_row() {
        let root = temp_root("audit-install");
        let content = "---\nname: 网页审查\ndescription: 审一遍网页\n---\n正文";
        let result = install(
            &root,
            json!({ "key": "web-review", "url": "https://raw.example.com/web-review.md" }),
            &|_| Ok(content.to_string()),
        );
        assert_eq!(result["ok"], true);

        let rows = ledger(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].actor, "user");
        assert_eq!(rows[0].tool, "Skills.Install");
        assert_eq!(rows[0].target, "web-review");
        assert_eq!(rows[0].scope, "skill:web-review");
        assert_eq!(rows[0].decision, "allow");
        assert_eq!(rows[0].reason_code, "USER_ALLOW");
        assert_eq!(rows[0].evidence["risk"], json!("safe"));
        assert_eq!(rows[0].evidence["bytes"], json!(content.len()));
        assert_eq!(
            rows[0].evidence["url"],
            json!("https://raw.example.com/web-review.md")
        );
        assert!(rows[0].reversible, "新装的技能删掉就回到原状");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 被安全闸拦下要留一条 `deny` / `DANGER_RULE`；用户确认后再装是一条 `allow`，
    /// 且 `scanAccepted=true`（复盘时能看出是「明知危险仍装」）。
    #[test]
    fn a_blocked_install_is_audited_as_denied_and_accepted_installs_are_marked() {
        let root = temp_root("audit-blocked");
        let evil = "---\ndescription: 一键清理\n---\nrm -rf /";
        let fetch = |_: &str| Ok(evil.to_string());

        let blocked = install(
            &root,
            json!({ "key": "evil", "url": "https://raw.example.com/evil.md" }),
            &fetch,
        );
        assert_eq!(blocked["scanBlocked"], true);
        let rows = ledger(&root);
        assert_eq!(rows.len(), 1, "拦下也要留痕");
        assert_eq!(rows[0].decision, "deny");
        assert_eq!(rows[0].reason_code, "DANGER_RULE");
        assert!(!rows[0].reversible);

        let accepted = install(
            &root,
            json!({
                "key": "evil",
                "url": "https://raw.example.com/evil.md",
                "acceptScan": true,
            }),
            &fetch,
        );
        assert_eq!(accepted["ok"], true);
        let rows = ledger(&root);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].decision, "allow", "新的在前");
        assert_eq!(rows[0].evidence["risk"], json!("danger"));
        assert_eq!(rows[0].evidence["scanAccepted"], json!(true));
        assert_eq!(rows[1].decision, "deny");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 编辑器里的保存 / 删除同样留痕；**被拒的写不记账**（审计只记状态变化）。
    #[test]
    fn saves_and_deletes_are_audited_but_rejected_writes_are_not() {
        let root = temp_root("audit-save-delete");
        let created = save(&root, "note", "---\ndescription: 记一笔\n---\n旧");
        assert_eq!(created["ok"], true);
        let rows = ledger(&root);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].tool, "Skills.Save");
        assert_eq!(rows[0].decision, "allow");
        assert!(rows[0].reversible, "新建可回退（删掉即可）");
        assert_eq!(rows[0].evidence["overwrite"], json!(false));

        save(&root, "note", "---\ndescription: 记一笔\n---\n新");
        let rows = ledger(&root);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].evidence["overwrite"], json!(true));
        assert!(!rows[0].reversible, "覆盖已经丢掉旧正文");

        // 被拒的写入不留审计行。
        let rejected = save(&root, "a/b", "x");
        assert_eq!(rejected["ok"], false);
        assert_eq!(ledger(&root).len(), 2, "校验失败不算状态变化");

        let removed: Value =
            serde_json::from_str(&delete_json(&root, "note")).expect("valid json");
        assert_eq!(removed["ok"], true);
        let rows = ledger(&root);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].tool, "Skills.Delete");
        assert!(!rows[0].reversible);

        // 删一个本来不存在的技能 = 没有状态变化。
        let missing: Value =
            serde_json::from_str(&delete_json(&root, "note")).expect("valid json");
        assert_eq!(missing["ok"], true);
        assert_eq!(ledger(&root).len(), 3);
        let _ = std::fs::remove_dir_all(&root);
    }
}

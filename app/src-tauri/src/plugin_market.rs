// src-tauri/src/plugin_market.rs
// 插件市场（L1，2026-09-21）：从 https 的 zip 安装第三方插件到固定目录。
//
// 目录约定：**<exe 根>\plugins\<id>\**（与 `skills\` / `tools\` 同级，都在安装根下，卸载随目录清掉）。
// 包里必须带一份清单 `lunac-plugin.json`（见 `PluginManifest`），入口是**已编译好的 ESM**（`import()`），
// 因为前端 CSP 是 `script-src 'self' 'unsafe-inline' https://asset.localhost` —— 插件代码只能经
// **asset 协议**从磁盘加载，**不能走 CDN**（同 ai-spec §3.7 的 KaTeX 缺陷是同一条约束）。
//
// **这份代码解压的是「可执行代码」，所以校验比 tools / skills 那两个先例更严**：
//   ① 只收 https（明文 http 的 zip 会被解压执行）；
//   ② 压缩包与**解压后总量**都有上限（zip bomb）；
//   ③ 逐条拒绝绝对路径、`..`、超长路径（路径穿越）；
//   ④ id 只允许安全字符（它直接当目录名）；
//   ⑤ 已存在则**拒绝**，不静默覆盖（覆盖别人的插件目录等于无声换代码）；
//   ⑥ 先解到 `.staging-*` 再改名进正式目录，失败即清理，不留半成品。
//
// 安全边界要诚实：这里做的是**防事故**（写坏路径、把包塞爆），**不是**防恶意 ——
// 插件是用户自己选择安装的可执行代码，装上就等同于本机权限（与 tools\ 的 shell handler 同族）。
// 界面上必须把「来源 + 这是可执行代码」显著写出来，别让用户以为它只是个配置。

use std::fs;
use std::io::{Read, Seek, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// 插件清单文件名（放在包根，也可放在 zip 里的单层子目录下 —— 见 `find_manifest`）。
pub const MANIFEST_FILE: &str = "lunac-plugin.json";
/// 压缩包大小上限。插件里常有引擎与图片，给得比 tools / skills 宽，但仍要拦住「几百 MB 的包」。
pub const MAX_ARCHIVE_BYTES: u64 = 32 * 1024 * 1024;
/// 解压后总量上限（zip bomb 防护）。
pub const MAX_TOTAL_BYTES: u64 = 192 * 1024 * 1024;
/// 条目数上限（防「十万个空文件」）。
pub const MAX_ENTRIES: usize = 4000;
/// id 长度上限（同时是目录名长度上限）。
const MAX_ID_CHARS: usize = 48;

/// 插件清单。除 `id` 外全部有默认值：缺字段的包也能装，但至少要能读出 id 与入口。
///
/// 字段刻意与前端 `Plugin` 契约（`app/src/plugins/registry.ts`）对齐：`keywords` 参与搜索匹配、
/// `icon` 是 emoji 兜底、`description` 是英文兜底（显示名与描述仍优先走 i18n 的 `plugin.<id>`）。
#[derive(Debug, Serialize, Deserialize, Clone, Default)]
pub struct PluginManifest {
    /// 插件 id，**同时是目录名** ⇒ 只允许 `[a-z0-9._-]`，不许以 `.` 开头
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub keywords: Vec<String>,
    #[serde(default)]
    pub icon: String,
    #[serde(default)]
    pub version: String,
    /// ESM 入口，相对清单所在目录，如 `index.js`。缺省 `index.js`。
    #[serde(default = "default_entry")]
    pub entry: String,
    /// 作者 / 仓库地址（界面上显示来源用）
    #[serde(default)]
    pub homepage: String,
}

fn default_entry() -> String {
    "index.js".to_string()
}

/// 列给前端的已安装插件（= 清单 + 落点，外加「能不能用」的判定）。
///
/// `rename_all = "camelCase"`：前端接口（`app/src/plugins/market.ts` 的 `MarketPluginInfo`）
/// 按 JS 习惯写 `entryPath`，别让调用方去记哪一个字段是 snake_case。
#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPlugin {
    pub id: String,
    pub name: String,
    pub description: String,
    pub keywords: Vec<String>,
    pub icon: String,
    pub version: String,
    pub entry: String,
    pub homepage: String,
    /// 插件目录绝对路径（前端用 `convertFileSrc` 拼出 ESM 入口 URL）
    pub dir: String,
    /// 入口文件的绝对路径（前端直接用它，免得自己拼相对路径）
    pub entry_path: String,
    /// 清单与入口都通过校验
    pub valid: bool,
    /// `valid = false` 时的原因（原样显示在面板上，不猜）
    pub error: String,
}

/// 插件根目录：`<exe 根>\plugins`（与 skills / tools 同级）。
pub fn plugins_dir() -> PathBuf {
    crate::storage::lunac_root_dir().join("plugins")
}

/// id 合法性 —— 它**直接当目录名**用，所以只允许安全字符。
///
/// 拒绝项：空、超长、非 `[a-z0-9._-]`、以 `.` 开头（避免与 `.staging-*` 之类的内部目录撞上）、
/// 含 `..`（防穿越）、含 Windows 保留字符（已被字符集挡住）。
pub fn is_safe_id(id: &str) -> bool {
    let id = id.trim();
    if id.is_empty() || id.chars().count() > MAX_ID_CHARS {
        return false;
    }
    if id.starts_with('.') || id.contains("..") {
        return false;
    }
    id.chars()
        .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_' || c == '.')
}

/// 入口必须是**相对**的、以 `.js` / `.mjs` 结尾、不含 `..`。
pub fn validate_entry(entry: &str) -> Result<(), String> {
    let e = entry.trim().replace('\\', "/");
    if e.is_empty() {
        return Err("清单里的 entry 是空的".into());
    }
    if e.starts_with('/') || e.contains(':') || e.split('/').any(|seg| seg == "..") {
        return Err(format!("entry 必须是包内相对路径：{entry}"));
    }
    if !(e.ends_with(".js") || e.ends_with(".mjs")) {
        return Err(format!("entry 必须是 .js / .mjs（ESM 模块）：{entry}"));
    }
    Ok(())
}

/// 解析并校验清单文本。**校验不过一个字段都不落盘**（调用方据此拒绝整包）。
pub fn parse_manifest(text: &str) -> Result<PluginManifest, String> {
    // BOM 容错：编辑器常写 BOM，而 serde_json 见 BOM 直接判非法（同 hooks.json 的处置）
    let text = text.trim_start_matches('\u{feff}');
    let m: PluginManifest =
        serde_json::from_str(text).map_err(|e| format!("插件清单不是合法 JSON：{e}"))?;
    if !is_safe_id(&m.id) {
        return Err(format!(
            "插件 id 非法（只允许小写字母/数字/-/_/.，且不能以 . 开头）：{}",
            m.id
        ));
    }
    validate_entry(&m.entry)?;
    Ok(m)
}

/// 把 zip 里的条目名规范化成 root 下的路径；**任何越界都返回 None**。
///
/// 判据（逐条，缺一不可）：非空、不是绝对路径（`/` 或 `\` 开头、含盘符 `:`）、
/// 分段里没有 `..`、没有空段（`a//b`）、分段不以空格或 `.` 结尾（Windows 会把这种名字悄悄改掉，
/// 让「写下去的文件」与「校验过的路径」不是同一个东西）。反斜杠统一按分隔符看（zip 规范用 `/`，
/// 但手工打的包常混用，而它们在 Windows 上都会被当成目录分隔符）。
pub fn safe_join(root: &Path, name: &str) -> Option<PathBuf> {
    let unified = name.replace('\\', "/");
    if unified.is_empty() || unified.starts_with('/') || unified.contains(':') {
        return None;
    }
    let mut out = root.to_path_buf();
    let mut depth = 0usize;
    for seg in unified.split('/') {
        if seg.is_empty() || seg == "." || seg == ".." {
            return None;
        }
        if seg.ends_with(' ') || seg.ends_with('.') {
            return None;
        }
        out.push(seg);
        depth += 1;
        if depth > 16 {
            return None;
        }
    }
    Some(out)
}

/// 解压 zip 到 `dest`（`dest` 必须已经存在或可被创建）。
///
/// `max_total` 是解压总量上限 —— 抽成参数只为让单测能用极小的上限造 zip bomb 场景，
/// 生产调用一律用 `MAX_TOTAL_BYTES`。
pub fn extract_zip<R: Read + Seek>(reader: R, dest: &Path, max_total: u64) -> Result<(), String> {
    let mut archive =
        zip::ZipArchive::new(reader).map_err(|e| format!("不是合法的 zip：{e}"))?;
    if archive.len() > MAX_ENTRIES {
        return Err(format!("包内条目过多（{} 个）", archive.len()));
    }
    fs::create_dir_all(dest).map_err(|e| format!("建目录失败：{e}"))?;
    let mut written: u64 = 0;
    for i in 0..archive.len() {
        let mut file = archive
            .by_index(i)
            .map_err(|e| format!("读第 {i} 个条目失败：{e}"))?;
        // 只收普通文件与目录 —— 符号链接 / 设备节点一律跳过（它们的落点是「别的地方」）
        if !file.is_file() && !file.is_dir() {
            continue;
        }
        let raw_name = file.name().to_string();
        let Some(path) = safe_join(dest, &raw_name) else {
            return Err(format!("包里含越界路径，已拒绝安装：{raw_name}"));
        };
        if file.is_dir() {
            fs::create_dir_all(&path).map_err(|e| format!("建目录失败：{e}"))?;
            continue;
        }
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| format!("建目录失败：{e}"))?;
        }
        let mut out = fs::File::create(&path).map_err(|e| format!("写文件失败：{e}"))?;
        // 分块拷贝 + 累计计数：不信 zip 头里声明的大小（那是包自己写的），按真实写出的字节算
        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = file.read(&mut buf).map_err(|e| format!("解压失败：{e}"))?;
            if n == 0 {
                break;
            }
            written += n as u64;
            if written > max_total {
                return Err(format!(
                    "解压后体积超过上限（{} MB），已拒绝安装",
                    max_total / 1024 / 1024
                ));
            }
            out.write_all(&buf[..n])
                .map_err(|e| format!("写文件失败：{e}"))?;
        }
    }
    Ok(())
}

/// 在解压出来的目录里找清单：**包根**或**唯一的一层子目录**（GitHub 的 zip 打包会给一个顶层目录）。
fn find_manifest(root: &Path) -> Result<PathBuf, String> {
    let direct = root.join(MANIFEST_FILE);
    if direct.is_file() {
        return Ok(direct);
    }
    let mut found: Option<PathBuf> = None;
    let entries = fs::read_dir(root).map_err(|e| format!("读目录失败：{e}"))?;
    for e in entries.flatten() {
        let p = e.path();
        if !p.is_dir() {
            continue;
        }
        let cand = p.join(MANIFEST_FILE);
        if cand.is_file() {
            // 有第二个就说明形状不唯一 —— 宁可拒绝，也不要猜用户装的是哪一个
            if found.is_some() {
                return Err(format!("包里有不止一份 {MANIFEST_FILE}，无法判断用哪份"));
            }
            found = Some(cand);
        }
    }
    found.ok_or_else(|| format!("包里没有 {MANIFEST_FILE}"))
}

/// 安装一份 zip 字节流：解到 staging → 校验清单 → 改名进 `<plugins_root>\<id>`。
///
/// 返回值是插件 id。**已存在同 id 的目录 ⇒ 直接报错**（不覆盖：静默换掉一个插件目录里的代码
/// 是最不该发生的事，用户想升级就先卸载）。
pub fn install_from_bytes(bytes: &[u8], plugins_root: &Path) -> Result<String, String> {
    if bytes.len() as u64 > MAX_ARCHIVE_BYTES {
        return Err(format!(
            "压缩包超过上限（{} MB）",
            MAX_ARCHIVE_BYTES / 1024 / 1024
        ));
    }
    fs::create_dir_all(plugins_root).map_err(|e| format!("建插件目录失败：{e}"))?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let staging = plugins_root.join(format!(".staging-{stamp}"));
    let _ = fs::remove_dir_all(&staging);
    // 从这里起，任何早退都要把 staging 清掉（否则会在插件目录里留一堆半成品）
    let result = (|| -> Result<String, String> {
        extract_zip(std::io::Cursor::new(bytes), &staging, MAX_TOTAL_BYTES)?;
        let manifest_path = find_manifest(&staging)?;
        let text = fs::read_to_string(&manifest_path)
            .map_err(|e| format!("读清单失败：{e}"))?;
        let manifest = parse_manifest(&text)?;
        let plugin_root = manifest_path
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| staging.clone());
        let entry_path = safe_join(&plugin_root, &manifest.entry)
            .ok_or_else(|| format!("entry 越界：{}", manifest.entry))?;
        if !entry_path.is_file() {
            return Err(format!(
                "清单里写的入口不存在：{}",
                manifest.entry
            ));
        }
        let target = plugins_root.join(&manifest.id);
        if target.exists() {
            return Err(format!(
                "插件“{}”已存在，请先卸载再安装",
                manifest.id
            ));
        }
        // 包的形状可能有两种：清单就在 staging 根（整包就是插件），或在 staging\<子目录>（GitHub zip）
        if plugin_root == staging {
            fs::rename(&staging, &target).map_err(|e| format!("落盘失败：{e}"))?;
        } else {
            // 先把子目录搬出来，再整体改名 —— 两步都在同一卷内
            let inner = plugins_root.join(format!(".staging-{stamp}-inner"));
            fs::rename(&plugin_root, &inner).map_err(|e| format!("落盘失败：{e}"))?;
            let _ = fs::remove_dir_all(&staging);
            fs::rename(&inner, &target).map_err(|e| format!("落盘失败：{e}"))?;
        }
        Ok(manifest.id.clone())
    })();
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

/// 扫插件目录，列出全部已安装插件（含「坏包」——它们也要在面板上可见，否则用户只会看到插件莫名其妙消失）。
pub fn list_installed(plugins_root: &Path) -> Vec<InstalledPlugin> {
    let mut out: Vec<InstalledPlugin> = Vec::new();
    let Ok(entries) = fs::read_dir(plugins_root) else {
        return out;
    };
    for e in entries.flatten() {
        let dir = e.path();
        let name = e.file_name().to_string_lossy().to_string();
        if !dir.is_dir() || name.starts_with('.') {
            continue;
        }
        let mut item = InstalledPlugin {
            id: name.clone(),
            name: name.clone(),
            description: String::new(),
            keywords: Vec::new(),
            icon: String::new(),
            version: String::new(),
            entry: String::new(),
            homepage: String::new(),
            dir: dir.to_string_lossy().to_string(),
            entry_path: String::new(),
            valid: false,
            error: String::new(),
        };
        match fs::read_to_string(dir.join(MANIFEST_FILE)) {
            Ok(text) => match parse_manifest(&text) {
                Ok(m) => {
                    item.id = m.id;
                    item.name = if m.name.trim().is_empty() {
                        name.clone()
                    } else {
                        m.name.clone()
                    };
                    item.description = m.description.clone();
                    item.keywords = m.keywords.clone();
                    item.icon = m.icon.clone();
                    item.version = m.version.clone();
                    item.entry = m.entry.clone();
                    item.homepage = m.homepage.clone();
                    match safe_join(&dir, &m.entry) {
                        Some(p) if p.is_file() => {
                            item.entry_path = p.to_string_lossy().to_string();
                            item.valid = true;
                        }
                        _ => item.error = format!("入口文件不存在：{}", m.entry),
                    }
                }
                Err(err) => item.error = err,
            },
            Err(_) => item.error = format!("缺少 {MANIFEST_FILE}"),
        }
        out.push(item);
    }
    // 顺序稳定（目录名升序）：面板每次重绘的顺序不该随 read_dir 抖动
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

/// 卸载：**只删 `<plugins_root>\<id>`**，且 id 必须安全、目录必须真的在插件根下（防穿越）。
pub fn uninstall(id: &str, plugins_root: &Path) -> Result<(), String> {
    if !is_safe_id(id) {
        return Err(format!("插件 id 非法：{id}"));
    }
    let dir = plugins_root.join(id);
    if !dir.is_dir() {
        return Err("插件不存在".into());
    }
    // 双保险：canonicalize 之后确认它确实落在插件根内（符号链接 / 手工造的怪目录都拦下）
    let real_root = plugins_root
        .canonicalize()
        .map_err(|e| format!("插件目录不可用：{e}"))?;
    let real_dir = dir.canonicalize().map_err(|e| format!("插件目录不可用：{e}"))?;
    if !real_dir.starts_with(&real_root) {
        return Err("插件目录不在插件根内，已拒绝删除".into());
    }
    fs::remove_dir_all(&real_dir).map_err(|e| format!("删除失败：{e}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use zip::write::SimpleFileOptions;

    fn tmp_root(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!(
            "lunac-test-plugins-{tag}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    /// 造一个 zip：`files` 是 (包内路径, 内容)
    fn zip_bytes(files: &[(&str, &str)]) -> Vec<u8> {
        let mut w = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
        for (name, body) in files {
            w.start_file(*name, opts).unwrap();
            w.write_all(body.as_bytes()).unwrap();
        }
        w.finish().unwrap().into_inner()
    }

    fn good_manifest(id: &str) -> String {
        format!(
            r#"{{"id":"{id}","name":"Demo","description":"d","keywords":["demo"],"icon":"x","version":"1.0.0","entry":"index.js"}}"#
        )
    }

    #[test]
    fn safe_join_rejects_traversal_and_absolute_paths() {
        let root = Path::new("C:\\plugins");
        assert!(safe_join(root, "index.js").is_some());
        assert!(safe_join(root, "sub/dir/a.js").is_some());
        // 越界的四种写法
        assert!(safe_join(root, "../evil.js").is_none());
        assert!(safe_join(root, "a/../../evil.js").is_none());
        assert!(safe_join(root, "/abs.js").is_none());
        assert!(safe_join(root, "C:/abs.js").is_none());
        // Windows 会悄悄改掉以点/空格结尾的分段 —— 校验过的路径与写下去的路径就不是同一个了
        assert!(safe_join(root, "a./b.js").is_none());
        assert!(safe_join(root, "a /b.js").is_none());
        // 反斜杠按分隔符看（手工打的包常混用）
        assert!(safe_join(root, "..\\evil.js").is_none());
    }

    #[test]
    fn manifest_requires_safe_id_and_js_entry() {
        assert!(parse_manifest(&good_manifest("demo-pet")).is_ok());
        // BOM 容错
        assert!(parse_manifest(&format!("\u{feff}{}", good_manifest("demo"))).is_ok());
        // id：大写 / 穿越 / 点开头 / 空
        assert!(parse_manifest(&good_manifest("Demo")).is_err());
        assert!(parse_manifest(&good_manifest("../evil")).is_err());
        assert!(parse_manifest(&good_manifest(".hidden")).is_err());
        assert!(parse_manifest(&good_manifest("")).is_err());
        // entry：绝对路径 / 穿越 / 不是 js
        for bad in ["/index.js", "../index.js", "index.ts", "index"] {
            let text = format!(
                r#"{{"id":"demo","entry":"{bad}"}}"#
            );
            assert!(parse_manifest(&text).is_err(), "entry={bad} 应被拒");
        }
        // 缺 entry ⇒ 用默认 index.js
        let m = parse_manifest(r#"{"id":"demo"}"#).unwrap();
        assert_eq!(m.entry, "index.js");
    }

    #[test]
    fn extract_zip_refuses_entries_outside_the_root() {
        let root = tmp_root("traversal");
        let bytes = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo")),
            ("../evil.js", "alert(1)"),
        ]);
        let err = extract_zip(std::io::Cursor::new(&bytes), &root, MAX_TOTAL_BYTES).unwrap_err();
        assert!(err.contains("越界路径"), "{err}");
        // 一个字节都不该落到 root 之外（root 的父目录里不能出现 evil.js）
        assert!(!root.parent().unwrap().join("evil.js").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn extract_zip_stops_when_unpacked_size_exceeds_the_cap() {
        let root = tmp_root("bomb");
        let big = "x".repeat(200 * 1024);
        let bytes = zip_bytes(&[("lunac-plugin.json", &good_manifest("demo")), ("index.js", &big)]);
        // 上限设成 1KB ⇒ 第二个文件写到一半就该被拦下
        let err = extract_zip(std::io::Cursor::new(&bytes), &root, 1024).unwrap_err();
        assert!(err.contains("超过上限"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_round_trips_and_refuses_duplicates_and_broken_entry() {
        let root = tmp_root("install");
        let bytes = zip_bytes(&[
            ("lunac-plugin.json", &good_manifest("demo-pet")),
            ("index.js", "export default { id: 'demo-pet' };"),
        ]);
        let id = install_from_bytes(&bytes, &root).unwrap();
        assert_eq!(id, "demo-pet");
        let list = list_installed(&root);
        assert_eq!(list.len(), 1);
        assert!(list[0].valid, "{:?}", list[0].error);
        assert!(list[0].entry_path.ends_with("index.js"));
        assert!(!list[0].entry_path.contains(".staging"));
        // 同一 id 再装一次 ⇒ 拒绝，且原目录不受影响
        let err = install_from_bytes(&bytes, &root).unwrap_err();
        assert!(err.contains("已存在"), "{err}");
        assert!(root.join("demo-pet").join("index.js").is_file());
        // 清单写的入口不存在 ⇒ 拒绝，且不留 staging 残渣
        let bad = zip_bytes(&[("lunac-plugin.json", &good_manifest("demo-two"))]);
        let err = install_from_bytes(&bad, &root).unwrap_err();
        assert!(err.contains("入口不存在"), "{err}");
        let leftovers: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().to_string())
            .filter(|n| n.starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "留下残渣：{leftovers:?}");
        // 卸载
        uninstall("demo-pet", &root).unwrap();
        assert!(list_installed(&root).is_empty());
        // 非法 id 不许删
        assert!(uninstall("../x", &root).is_err());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_accepts_a_single_top_level_folder() {
        let root = tmp_root("github-shape");
        let bytes = zip_bytes(&[
            ("LunacPet/lunac-plugin.json", &good_manifest("lunac-pet")),
            ("LunacPet/index.js", "export default {};"),
        ]);
        assert_eq!(install_from_bytes(&bytes, &root).unwrap(), "lunac-pet");
        let list = list_installed(&root);
        assert!(list[0].valid, "{:?}", list[0].error);
        assert!(root.join("lunac-pet").join("index.js").is_file());
        assert!(!root.join("lunac-pet").join("LunacPet").exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn list_reports_a_broken_package_instead_of_hiding_it() {
        let root = tmp_root("broken");
        fs::create_dir_all(root.join("half-installed")).unwrap();
        fs::write(root.join("half-installed").join(MANIFEST_FILE), "{ not json").unwrap();
        // 内部目录（以 . 开头）不进清单
        fs::create_dir_all(root.join(".staging-1")).unwrap();
        let list = list_installed(&root);
        assert_eq!(list.len(), 1);
        assert!(!list[0].valid);
        assert!(list[0].error.contains("JSON"), "{:?}", list[0].error);
        let _ = fs::remove_dir_all(&root);
    }
}

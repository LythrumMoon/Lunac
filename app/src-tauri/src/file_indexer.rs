// file_indexer.rs
// 「详细搜索」的文件索引 —— 只服务主窗口双击进入的那个大界面（见 docs/ai-spec.md §2.1.2）。
//
// 与 app_indexer 同一套纪律（见项目规范「应用列表」硬约束）：
//   · 唯一存储是 <exe 根>\temp\file-index-cache.json，**搜索路径只读内存索引、永不扫盘**；
//     扫盘只发生在后台线程（启动 600ms 后 / 手动重建），写回必须原子（临时文件 + rename）。
//   · 索引未就绪时搜索返回空 + `scanning`，绝不在搜索线程里同步扫盘 —— 那是「一搜就卡」的根源。
//
// 扫描范围（2026-09-15 实测定的口径）：
//   系统盘（%SystemDrive%）→ 只扫 %USERPROFILE%；其它固定盘 → 全盘扫（深度受限）。
//   实测本机用户目录就有 19.6 万条，所以**存储形态必须紧凑**：
//   不存整条路径，存「目录表下标 + 文件名」（同目录几十个文件共享一份前缀），
//   也不存 size / ext（ext 展示时从文件名现推，size 目前没人用）。
//   全路径只在**返回命中的那几条**时拼回来（`FileHit::path`）。
//
// 条数上限 MAX_ENTRIES、深度上限 MAX_DEPTH、跳过 SKIP_DIRS 里的系统/开发重目录。

use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime};

const INDEX_VERSION: u32 = 2;
/// 索引条数上限 —— 到了就停止遍历（防内存/缓存文件无界）
const MAX_ENTRIES: usize = 300_000;
/// 目录深度上限（相对根）
const MAX_DEPTH: usize = 12;
/// 缓存多久算过期。**24 小时**：扫一次本机约 10~20s，不能每次启动都重扫
/// （开机自启场景尤其不能）。想立刻纳新文件 → UI 上的「重建索引」按钮。
const STALE_AFTER: Duration = Duration::from_secs(24 * 3600);
/// 结果条数上限（前端 limit 的硬上限）
const MAX_RESULTS: usize = 200;
/// 模糊（子序列）/ 拼音兜底的查询长度上限：更长的查询靠模糊命中只会是噪声。
const FUZZY_MAX_QUERY_LEN: usize = 12;
/// 拼音兜底**每次搜索最多转换多少个文件名**。`pinyin` crate 的转换要分配字符串，
/// 而索引上限是 30 万条 —— 不封顶的话每次击键都会在工作线程里白烧几十毫秒。
/// 封顶后 `zjl` 这类拼音查询仍能命中（名字里带汉字的条目在索引里本来就占少数）。
const PINYIN_SCAN_CAP: usize = 30_000;

/// 跳过的目录名（全部小写比较）。这些都是「扫了没用却极贵」的重目录。
const SKIP_DIRS: &[&str] = &[
    "$recycle.bin",
    "system volume information",
    "$windows.~bt",
    "$windows.~ws",
    "windows",
    "winsxs",
    "node_modules",
    ".git",
    ".svn",
    ".hg",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".venv",
    "venv",
    "env",
    ".cache",
    ".gradle",
    ".m2",
    "appdata",
    "programdata",
    "temp",
    "tmp",
    "cache",
    "packages",
];

/// 文件分类。用 `u8` 枚举存 —— 20 万条量级下字符串字段太贵。
/// 落盘为小写字符串（`"document"`），保持缓存文件可读。
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Folder,
    Document,
    Image,
    Video,
    Audio,
    Archive,
    Program,
    Other,
}

impl Kind {
    fn for_ext(ext: &str) -> Kind {
        match ext {
            "doc" | "docx" | "pdf" | "txt" | "md" | "rtf" | "odt" | "xls" | "xlsx" | "csv" | "ppt"
            | "pptx" | "odp" | "ods" | "epub" | "json" | "xml" | "yaml" | "yml" | "log" => {
                Kind::Document
            }
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "svg" | "ico" | "tif" | "tiff"
            | "heic" | "raw" | "psd" => Kind::Image,
            "mp4" | "mkv" | "avi" | "mov" | "wmv" | "flv" | "webm" | "m4v" | "mpg" | "mpeg" => {
                Kind::Video
            }
            "mp3" | "wav" | "flac" | "aac" | "ogg" | "m4a" | "wma" | "opus" | "mid" => Kind::Audio,
            "zip" | "rar" | "7z" | "tar" | "gz" | "bz2" | "xz" | "iso" | "cab" => Kind::Archive,
            "exe" | "msi" | "bat" | "cmd" | "ps1" | "lnk" | "com" | "msix" | "appx" | "jar" => {
                Kind::Program
            }
            _ => Kind::Other,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Kind::Folder => "folder",
            Kind::Document => "document",
            Kind::Image => "image",
            Kind::Video => "video",
            Kind::Audio => "audio",
            Kind::Archive => "archive",
            Kind::Program => "program",
            Kind::Other => "other",
        }
    }
}

/// 索引里的存储形态（紧凑）。路径不在这里，见 `dirs` 表。
#[derive(Serialize, Deserialize, Clone, Debug)]
struct FileEntry {
    name: String,
    /// 父目录在 `dirs` 里的下标
    dir: u32,
    kind: Kind,
    /// 修改时间（毫秒时间戳；读不到为 0）
    modified: u64,
}

/// 返回给前端的命中项（路径与扩展名在这里才拼/推出来）。
#[derive(Serialize, Clone, Debug)]
pub struct FileHit {
    pub name: String,
    pub path: String,
    pub kind: String,
    pub ext: String,
    pub modified: u64,
}

#[derive(Serialize, Deserialize, Default)]
struct PersistedIndex {
    version: u32,
    saved_ms: u64,
    /// 是否因为 MAX_ENTRIES 提前收工（UI 如实告知"索引不完整"）
    #[serde(default)]
    truncated: bool,
    #[serde(default)]
    roots: Vec<String>,
    /// 目录表：`FileEntry::dir` 指向它
    #[serde(default)]
    dirs: Vec<String>,
    files: Vec<FileEntry>,
}

#[derive(Serialize, Clone)]
pub struct IndexStatus {
    /// 已索引条数
    pub count: usize,
    /// 是否正在扫盘
    pub scanning: bool,
    /// 索引落盘时间（毫秒；0 = 还没有索引文件）
    pub saved_ms: u64,
    /// 是否因上限截断
    pub truncated: bool,
    /// 实际扫描的根目录（透明化：用户能看到我们扫了哪儿）
    pub roots: Vec<String>,
}

struct IndexState {
    dirs: Vec<String>,
    files: Vec<FileEntry>,
    saved_ms: u64,
    truncated: bool,
    roots: Vec<String>,
    /// 索引是否已从磁盘载入过（未载入 = 搜索返回空）
    loaded: bool,
    scanning: bool,
}

static INDEX: OnceLock<RwLock<IndexState>> = OnceLock::new();
/// 扫盘互斥：启动重扫与手动重建可能撞车，同一时刻只允许一个线程遍历磁盘。
static SCAN_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn state() -> &'static RwLock<IndexState> {
    INDEX.get_or_init(|| {
        RwLock::new(IndexState {
            dirs: Vec::new(),
            files: Vec::new(),
            saved_ms: 0,
            truncated: false,
            roots: Vec::new(),
            loaded: false,
            scanning: false,
        })
    })
}

/// 缓存文件：<exe 根>\temp\file-index-cache.json（与 app-index-cache.json 同目录）。
pub fn cache_path() -> PathBuf {
    crate::storage::lunac_root_dir().join("temp").join("file-index-cache.json")
}

// ── 启动与刷新 ───────────────────────────────────────────────────

/// 启动时调用：后台线程载入缓存 → 过期（>24h）则重扫。**不阻塞启动**。
pub fn init() {
    std::thread::spawn(|| {
        std::thread::sleep(Duration::from_millis(600));
        load_cache();
        if cache_is_stale() {
            scan_and_save();
        }
    });
}

fn cache_is_stale() -> bool {
    let st = state().read().ok();
    let saved = st.as_ref().map(|s| s.saved_ms).unwrap_or(0);
    let loaded = st.as_ref().map(|s| s.loaded).unwrap_or(false);
    if !loaded || saved == 0 {
        return true;
    }
    now_ms().saturating_sub(saved) > STALE_AFTER.as_millis() as u64
}

/// 强制重扫（UI 上的「重建索引」）。
pub fn refresh_in_background() {
    std::thread::spawn(scan_and_save);
}

fn load_cache() {
    let path = cache_path();
    let Ok(text) = fs::read_to_string(&path) else {
        return;
    };
    match serde_json::from_str::<PersistedIndex>(&text) {
        Ok(mut idx) if idx.version == INDEX_VERSION => {
            let count = idx.files.len();
            if let Ok(mut st) = state().write() {
                st.files = std::mem::take(&mut idx.files);
                st.dirs = std::mem::take(&mut idx.dirs);
                st.saved_ms = idx.saved_ms;
                st.truncated = idx.truncated;
                st.roots = idx.roots;
                st.loaded = true;
            }
            crate::log::info(format!("文件索引已载入: {count} 条（{}）", path.display()));
        }
        Ok(other) => {
            // 版本不符：当作没有索引，等重扫覆盖（不在这里删文件）
            crate::log::info(format!("文件索引版本不符（{}≠{INDEX_VERSION}），将重建", other.version));
        }
        Err(e) => crate::log::warn(format!("文件索引解析失败: {e}")),
    }
}

fn scan_and_save() {
    // 同一时刻只允许一个扫盘线程（启动重扫与手动重建可能撞车）
    let scan_mutex = SCAN_LOCK.get_or_init(|| Mutex::new(()));
    let Ok(_guard) = scan_mutex.try_lock() else {
        return; // 已有一个在扫 —— 直接返回，不排队
    };
    if let Ok(mut st) = state().write() {
        st.scanning = true;
    }

    let roots = scan_roots();
    let started = SystemTime::now();
    let mut dir_table: Vec<String> = Vec::new();
    let mut dir_index: HashMap<String, u32> = HashMap::new();
    let mut files: Vec<FileEntry> = Vec::new();
    let mut truncated = false;
    let mut per_root: Vec<(String, usize)> = Vec::new();

    'outer: for root in &roots {
        // **广度优先**：截断是常态（实测用户目录就 19.6 万条），BFS 保证浅层文件
        // 先入索引 —— 深度优先会把预算全喂给第一个子目录，用户看到的是「随机缺文件」。
        let mut queue: VecDeque<(PathBuf, usize)> = VecDeque::new();
        queue.push_back((root.clone(), 0usize));
        while let Some((dir, depth)) = queue.pop_front() {
            if files.len() >= MAX_ENTRIES {
                truncated = true;
                break 'outer;
            }
            let Ok(rd) = fs::read_dir(&dir) else { continue };
            let dir_key = dir.to_string_lossy().to_string();
            for entry in rd.flatten() {
                let name = entry.file_name().to_string_lossy().to_string();
                let Ok(meta) = entry.metadata() else { continue };
                let is_dir = meta.is_dir();
                if is_dir {
                    if depth < MAX_DEPTH && !skip_dir(&name) && !name.starts_with('.') {
                        queue.push_back((entry.path(), depth + 1));
                    }
                } else if !meta.is_file() {
                    continue; // 设备/管道/符号链接等一律不要
                }
                let kind = if is_dir {
                    Kind::Folder
                } else {
                    Kind::for_ext(&extension_of(&name))
                };
                // 目录表按需登记（同一个父目录的几十个文件只存一份路径）
                let dir_id = match dir_index.get(&dir_key) {
                    Some(id) => *id,
                    None => {
                        let id = dir_table.len() as u32;
                        dir_table.push(dir_key.clone());
                        dir_index.insert(dir_key.clone(), id);
                        id
                    }
                };
                files.push(FileEntry {
                    name,
                    dir: dir_id,
                    kind,
                    modified: modified_ms(&meta),
                });
            }
        }
        // 每个根扫出多少条 —— 达到上限时靠这行定位「哪个盘把预算吃完了」
        per_root.push((root.to_string_lossy().to_string(), files.len()));
    }

    let elapsed = started.elapsed().map(|d| d.as_millis()).unwrap_or(0);
    let count = files.len();
    let dir_count = dir_table.len();
    let saved_ms = now_ms();
    let root_strings: Vec<String> = roots.iter().map(|p| p.to_string_lossy().to_string()).collect();
    let payload = PersistedIndex {
        version: INDEX_VERSION,
        saved_ms,
        truncated,
        roots: root_strings.clone(),
        dirs: dir_table,
        files,
    };
    match serde_json::to_string(&payload) {
        Ok(text) => {
            if let Err(e) = write_atomic(&cache_path(), &text) {
                crate::log::warn(format!("文件索引写盘失败: {e}"));
            }
        }
        Err(e) => crate::log::warn(format!("文件索引序列化失败: {e}")),
    }
    // 落盘之后再把结果搬进内存（不 clone 整份索引 —— 几十万条 clone 一次是几十 MB）
    if let Ok(mut st) = state().write() {
        st.files = payload.files;
        st.dirs = payload.dirs;
        st.saved_ms = saved_ms;
        st.truncated = truncated;
        st.roots = root_strings;
        st.loaded = true;
        st.scanning = false;
    }
    crate::log::info(format!(
        "文件索引已重建: {count} 条 / {dir_count} 个目录 / 根 {} 个{}{elapsed}ms",
        roots.len(),
        if truncated { "（已达上限，索引不完整）" } else { "，耗时 " }
    ));
    crate::log::info(format!(
        "文件索引各根累计: {}",
        per_root
            .iter()
            .map(|(r, n)| format!("{r}={n}"))
            .collect::<Vec<_>>()
            .join(" ")
    ));
}

/// 原子写：先写临时文件再 rename，避免半个 JSON 被下次启动读进去。
fn write_atomic(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, text)?;
    fs::rename(&tmp, path)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn modified_ms(meta: &fs::Metadata) -> u64 {
    meta.modified()
        .ok()
        .and_then(|t| t.duration_since(SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ── 扫描根目录 ───────────────────────────────────────────────────

fn scan_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(profile) = std::env::var("USERPROFILE") {
        let p = PathBuf::from(profile);
        if p.is_dir() {
            roots.push(p);
        }
    }
    let sys_drive = std::env::var("SystemDrive")
        .map(|d| d.to_ascii_uppercase())
        .unwrap_or_else(|_| "C:".into());
    for drive in fixed_drives() {
        // 系统盘只扫用户目录（Windows / Program Files 里没有用户要找的文件，却最贵）
        if drive.to_ascii_uppercase().starts_with(&sys_drive) {
            continue;
        }
        roots.push(PathBuf::from(drive));
    }
    roots
}

/// 固定磁盘列表（GetDriveTypeW == DRIVE_FIXED）。手写 FFI，与 hotkey.rs 同约定，不引依赖。
#[cfg(target_os = "windows")]
fn fixed_drives() -> Vec<String> {
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "kernel32")]
    extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_FIXED: u32 = 3;

    let mut out = Vec::new();
    for letter in b'A'..=b'Z' {
        let root = format!("{}:\\", letter as char);
        let wide: Vec<u16> = std::ffi::OsStr::new(&root)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        if unsafe { GetDriveTypeW(wide.as_ptr()) } == DRIVE_FIXED {
            out.push(root);
        }
    }
    out
}

#[cfg(not(target_os = "windows"))]
fn fixed_drives() -> Vec<String> {
    Vec::new()
}

fn skip_dir(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SKIP_DIRS.contains(&lower.as_str())
}

// ── 匹配（全部大小写无关、**零分配**，见文件头注释）────────────────

fn extension_of(name: &str) -> String {
    Path::new(name)
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

fn eq_ci(hay: &str, needle: &str) -> bool {
    hay.len() == needle.len() && hay.as_bytes().eq_ignore_ascii_case(needle.as_bytes())
}

fn starts_with_ci(hay: &str, needle: &str) -> bool {
    hay.as_bytes()
        .get(..needle.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(needle.as_bytes()))
}

/// 大小写无关的子串位置（字节下标）。ASCII 折叠，非 ASCII 逐字节比较
/// —— 对 CJK 查询而言等价（本来就没有大小写）。
fn find_ci(hay: &str, needle: &str) -> Option<usize> {
    let h = hay.as_bytes();
    let n = needle.as_bytes();
    if n.is_empty() {
        return Some(0);
    }
    if n.len() > h.len() {
        return None;
    }
    h.windows(n.len()).position(|w| w.eq_ignore_ascii_case(n))
}

/// 文件名打分（**纯函数**，零分配）。语义：精确 > 前缀 > 包含 > 多词全中。
/// 空查询返回 0 —— 此时排序退化成「按修改时间倒序」= 最近文件（Win+S 的空态）。
fn score_name(name: &str, tokens: &[&str]) -> Option<i32> {
    if tokens.is_empty() {
        return Some(0);
    }
    if tokens.len() == 1 {
        let q = tokens[0];
        if eq_ci(name, q) {
            return Some(1000);
        }
        if starts_with_ci(name, q) {
            return Some(700 - name.len().min(60) as i32);
        }
        if let Some(pos) = find_ci(name, q) {
            return Some(500 - pos.min(200) as i32);
        }
        return None;
    }
    // 多词：全部出现在文件名里即可（顺序无关）
    if tokens.iter().all(|t| find_ci(name, t).is_some()) {
        return Some(400 - name.len().min(100) as i32);
    }
    None
}

/// 子序列（模糊）匹配打分 —— 查询的字符按**顺序**出现在名字里即可命中，允许中间插字。
/// `zjl` 能命中「最近记录列表」。返回 `None` = 不构成子序列。
///
/// 分数刻意落在 200 一档（低于「包含」的 500 一档）：模糊命中永远是兜底，
/// 不能把精确命中的结果挤下去。跨度越大、名字越长扣分越多。
fn score_subsequence(name: &str, q: &str) -> Option<i32> {
    if q.is_empty() {
        return None;
    }
    let mut want = q.chars();
    let mut cur = want.next();
    let mut first = 0usize;
    let mut last = 0usize;
    let mut matched = 0usize;
    let mut total = 0usize;
    for (i, c) in name.chars().enumerate() {
        total = i + 1;
        let Some(w) = cur else { break };
        if c.eq_ignore_ascii_case(&w) {
            if matched == 0 {
                first = i;
            }
            last = i;
            matched += 1;
            cur = want.next();
        }
    }
    if cur.is_some() {
        return None; // 还有没匹配上的字符
    }
    let span = last - first + 1;
    let gaps = span - matched;
    Some(200 - (gaps as i32).min(80) - (total.min(80) as i32) / 4)
}

/// 拼音兜底打分：名字里有汉字时，用全拼 / 首字母各试一遍。
/// 分数落在 300 一档（低于「包含」的 500）：拼音是「猜用户想打什么」，不是直接匹配。
///
/// **首字母整体比全拼低一档**（340 / 300）：「weixin」命中「微信截图」比「wx」命中更确定，
/// 而 `wx` 这类两字母首字母几乎能匹配一大片中文名，不给它降档就会把全拼命中盖掉。
///
/// 只对含汉字的条目生效（`has_chinese` 先判一次），所以纯英文文件名零开销。
fn score_pinyin(name: &str, q: &str) -> Option<i32> {
    if !crate::app_indexer::has_chinese(name) {
        return None;
    }
    let tokens = crate::app_indexer::generate_pinyin_tokens(name);
    let mut best: Option<i32> = None;
    for (idx, t) in tokens.iter().enumerate() {
        // tokens[0] = 全拼，tokens[1]（若存在）= 首字母
        let base = if idx == 0 { 340 } else { 300 };
        let s = if eq_ci(t, q) {
            base
        } else if starts_with_ci(t, q) {
            base - 20 - t.len().min(40) as i32
        } else if let Some(pos) = find_ci(t, q) {
            base - 60 - pos.min(60) as i32
        } else {
            continue;
        };
        best = Some(best.map_or(s, |b: i32| b.max(s)));
    }
    best
}

/// 在给定集合上排序取前 N（纯函数，单测用）。`dirs` 提供目录表以拼回完整路径。
///
/// 两轮：**第一轮**是精确 / 前缀 / 包含 / 多词全中（语义与 2026-09-15 定稿一致，
/// 零分配）；只有第一轮**没凑够 limit** 时才跑**第二轮**兜底 —— 模糊子序列 + 拼音。
/// 这条门控很重要：单字母查询在第一轮就已经填满 200 条，不该再为它多扫一遍全表。
fn rank(
    dirs: &[String],
    entries: &[FileEntry],
    query: &str,
    kind: Option<&str>,
    limit: usize,
) -> Vec<FileHit> {
    let raw = query.trim();
    let tokens: Vec<&str> = raw.split_whitespace().collect();
    let wanted = kind.filter(|k| !k.is_empty());
    // 命中项的 `entries` 下标（用于第二轮去重，避免同一条被两轮各推一次）
    let mut hits: Vec<(i32, usize)> = Vec::new();
    let mut seen: std::collections::HashSet<usize> = std::collections::HashSet::new();
    for (i, e) in entries.iter().enumerate() {
        if let Some(k) = wanted {
            if e.kind.as_str() != k {
                continue;
            }
        }
        if let Some(score) = score_name(&e.name, &tokens) {
            seen.insert(i);
            hits.push((score, i));
        }
    }

    // ── 第二轮：模糊（子序列）+ 拼音兜底 ─────────────────────────────
    if !tokens.is_empty() && hits.len() < limit {
        let tokens_l: Vec<String> = tokens.iter().map(|t| t.to_lowercase()).collect();
        let single = tokens_l.len() == 1;
        let q_all = &tokens_l[0];
        // 拼音只在「单 token + 纯 ASCII + 长度合理」时试 —— 多词/带标点的查询
        // 走拼音没有意义，徒增开销。
        let allow_pinyin =
            single && q_all.len() >= 2 && q_all.len() <= FUZZY_MAX_QUERY_LEN
                && q_all.chars().all(|c| c.is_ascii_alphanumeric());
        let mut pinyin_budget = PINYIN_SCAN_CAP;
        for (i, e) in entries.iter().enumerate() {
            if seen.contains(&i) {
                continue;
            }
            if let Some(k) = wanted {
                if e.kind.as_str() != k {
                    continue;
                }
            }
            let mut score = if single {
                score_subsequence(&e.name, q_all)
            } else {
                // 多词：每个词都必须是子序列（与第一轮的「多词全中」同语义）
                let mut total = 0i32;
                let mut ok = true;
                for t in &tokens_l {
                    match score_subsequence(&e.name, t) {
                        Some(v) => total += v,
                        None => {
                            ok = false;
                            break;
                        }
                    }
                }
                if ok { Some(total / tokens_l.len() as i32) } else { None }
            };
            if score.is_none() && allow_pinyin && pinyin_budget > 0 {
                pinyin_budget -= 1;
                score = score_pinyin(&e.name, q_all);
            }
            if let Some(s) = score {
                seen.insert(i);
                hits.push((s, i));
            }
        }
    }

    // 同分时新修改的排前面（空查询=「最近文件」，就靠 modified 排序）
    hits.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then_with(|| entries[b.1].modified.cmp(&entries[a.1].modified))
    });
    hits.into_iter()
        .take(limit)
        .map(|(_, i)| {
            let e = &entries[i];
            let dir = dirs.get(e.dir as usize).map(String::as_str).unwrap_or("");
            FileHit {
                name: e.name.clone(),
                path: join_path(dir, &e.name),
                kind: e.kind.as_str().to_string(),
                ext: if e.kind == Kind::Folder { String::new() } else { extension_of(&e.name) },
                modified: e.modified,
            }
        })
        .collect()
}

fn join_path(dir: &str, name: &str) -> String {
    if dir.is_empty() {
        return name.to_string();
    }
    if dir.ends_with('\\') || dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}\\{name}")
    }
}

/// 搜索（只读内存索引，永不同步扫盘）。
pub fn search(query: &str, kind: Option<&str>, limit: usize) -> Vec<FileHit> {
    let limit = clamp_limit(limit);
    let Ok(st) = state().read() else { return Vec::new() };
    if !st.loaded {
        return Vec::new();
    }
    rank(&st.dirs, &st.files, query, kind, limit)
}

/// 结果条数夹取：前端传 0 / 超大值都不能把内存拖垮。
fn clamp_limit(limit: usize) -> usize {
    limit.clamp(1, MAX_RESULTS)
}

pub fn status() -> IndexStatus {
    match state().read() {
        Ok(st) => IndexStatus {
            count: st.files.len(),
            scanning: st.scanning,
            saved_ms: st.saved_ms,
            truncated: st.truncated,
            roots: st.roots.clone(),
        },
        Err(_) => IndexStatus {
            count: 0,
            scanning: false,
            saved_ms: 0,
            truncated: false,
            roots: Vec::new(),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(name: &str, kind: Kind, modified: u64) -> (String, FileEntry) {
        (
            r"C:\a".to_string(),
            FileEntry {
                name: name.into(),
                dir: 0,
                kind,
                modified,
            },
        )
    }

    fn rank_names(entries: &[(String, FileEntry)], query: &str, kind: Option<&str>) -> Vec<String> {
        let dirs: Vec<String> = vec![r"C:\a".to_string()];
        let files: Vec<FileEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
        rank(&dirs, &files, query, kind, 10).into_iter().map(|h| h.name).collect()
    }

    #[test]
    fn extension_maps_to_the_documented_kinds() {
        assert_eq!(Kind::for_ext("docx"), Kind::Document);
        assert_eq!(Kind::for_ext("png"), Kind::Image);
        assert_eq!(Kind::for_ext("mp4"), Kind::Video);
        assert_eq!(Kind::for_ext("flac"), Kind::Audio);
        assert_eq!(Kind::for_ext("7z"), Kind::Archive);
        assert_eq!(Kind::for_ext("exe"), Kind::Program);
        assert_eq!(Kind::for_ext("xyz"), Kind::Other);
        assert_eq!(Kind::for_ext(""), Kind::Other);
        assert_eq!(extension_of("报告.PDF"), "pdf", "扩展名必须小写归一");
    }

    #[test]
    fn heavy_directories_are_skipped() {
        for d in ["node_modules", "AppData", ".git", "target", "Windows"] {
            assert!(skip_dir(d), "{d} 应跳过");
        }
        assert!(!skip_dir("Documents"));
        assert!(!skip_dir("我的文档"));
    }

    #[test]
    fn case_insensitive_matching_without_allocation() {
        assert!(eq_ci("Report.DOCX", "report.docx"));
        assert!(starts_with_ci("Report.docx", "REPORT"));
        assert_eq!(find_ci("MyReport.docx", "report"), Some(2));
        assert_eq!(find_ci("abc", "abcd"), None);
        assert_eq!(find_ci("季度报告.xlsx", "报告"), Some(6), "中文按字节定位");
    }

    /// 精确 > 前缀 > 包含
    #[test]
    fn exact_name_beats_prefix_beats_contains() {
        let entries = vec![
            hit("report-final.docx", Kind::Document, 1),
            hit("report.docx", Kind::Document, 1),
            hit("report", Kind::Folder, 1),
        ];
        assert_eq!(
            rank_names(&entries, "report", None),
            vec!["report", "report.docx", "report-final.docx"]
        );
    }

    #[test]
    fn type_filter_and_multi_token() {
        let entries = vec![
            hit("季度报告.xlsx", Kind::Document, 5),
            hit("季度报告.png", Kind::Image, 9),
        ];
        assert_eq!(rank_names(&entries, "季度 报告", Some("document")), vec!["季度报告.xlsx"]);
        assert_eq!(rank_names(&entries, "报告 季度", None).len(), 2, "多词顺序无关");
    }

    /// 空查询 = 最近文件（按修改时间倒序）
    #[test]
    fn empty_query_lists_recent_first() {
        let entries = vec![
            hit("old.txt", Kind::Document, 100),
            hit("new.txt", Kind::Document, 900),
        ];
        assert_eq!(rank_names(&entries, "", None), vec!["new.txt", "old.txt"]);
        assert_eq!(rank_names(&entries, "   ", None).len(), 2, "纯空白等同空查询");
    }

    /// 路径由「目录表 + 文件名」拼回，且只在返回命中项时才算
    #[test]
    fn hits_carry_a_reconstructed_path() {
        let dirs = vec![r"C:\Users\me\Desktop".to_string()];
        let files = vec![FileEntry {
            name: "报告.docx".into(),
            dir: 0,
            kind: Kind::Document,
            modified: 7,
        }];
        let got = rank(&dirs, &files, "报告", None, 5);
        assert_eq!(got[0].path, r"C:\Users\me\Desktop\报告.docx");
        assert_eq!(got[0].ext, "docx");
        assert_eq!(got[0].kind, "document");
    }

    #[test]
    fn limit_is_respected_and_capped() {
        let entries: Vec<(String, FileEntry)> = (0..50)
            .map(|i| hit(&format!("f{i}.txt"), Kind::Document, i))
            .collect();
        let dirs = vec![r"C:\a".to_string()];
        let files: Vec<FileEntry> = entries.iter().map(|(_, e)| e.clone()).collect();
        assert_eq!(rank(&dirs, &files, "f", None, 8).len(), 8);
        assert!(rank(&dirs, &files, "f", None, 0).is_empty(), "rank 本身不管夹取");
        // 夹取（search 用）至少给 1、最多 MAX_RESULTS
        assert_eq!(clamp_limit(0), 1);
        assert_eq!(clamp_limit(usize::MAX), MAX_RESULTS);
        assert_eq!(clamp_limit(30), 30);
    }

    /// 子序列打分：顺序命中即算，顺序不对/字符缺失不命中；且**永远低于「包含」档（500）**
    #[test]
    fn subsequence_matching_is_a_fallback_tier() {
        assert!(score_subsequence("最近记录列表.docx", "zjl").is_none(), "中文名不吃 ASCII 子序列");
        assert!(score_subsequence("recent-journal-list.md", "zjl").is_none(), "没有 z 就不该命中");
        let s = score_subsequence("Zebra Journal List.md", "zjl").expect("z-j-l 按顺序出现");
        assert!((100..200).contains(&s), "子序列必须落在包含档之下，实测 {s}");
        assert!(s < 500, "模糊命中不得盖过包含命中");
        assert!(score_subsequence("abc", "").is_none(), "空查询不算子序列命中");
    }

    /// 拼音兜底：全拼（weixin）与首字母（wx）都能命中含汉字的文件名，全拼更确定
    #[test]
    fn pinyin_fallback_matches_full_and_initials() {
        let full = score_pinyin("微信截图.png", "weixin").expect("全拼应命中");
        let initial = score_pinyin("微信截图.png", "wx").expect("首字母应命中");
        assert!(full > initial, "全拼比首字母更确定：{full} vs {initial}");
        assert!(full < 500, "拼音是兜底档，不得盖过包含命中");
        assert!(score_pinyin("report.docx", "weixin").is_none(), "无汉字不产生拼音命中");
    }

    /// 第一轮没凑够才跑兜底：精确/前缀/包含优先，模糊只补空缺
    #[test]
    fn fuzzy_only_fills_the_gap_left_by_exact_passes() {
        let entries = vec![
            hit("report.docx", Kind::Document, 1),
            hit("recent-journal-list.md", Kind::Document, 2),
        ];
        // "report" 有精确命中；另一条既不含 report、也不是 report 的子序列 → 只出一条
        assert_eq!(rank_names(&entries, "report", None), vec!["report.docx"]);
        // "rjl" 第一轮零命中 → 走子序列兜底，命中 recent-journal-list
        assert_eq!(rank_names(&entries, "rjl", None), vec!["recent-journal-list.md"]);
        // 兜底轮跑完后，精确命中的那条仍排在模糊命中的前面
        let both = vec![
            hit("rjl-report.docx", Kind::Document, 1),
            hit("recent-journal-list.md", Kind::Document, 2),
        ];
        assert_eq!(
            rank_names(&both, "rjl", None)[0],
            "rjl-report.docx",
            "前缀命中必须压过子序列命中"
        );
    }

    /// 拼音兜底走的是同一轮门控（`zjt` 这种查询在第一轮必然零命中）
    #[test]
    fn pinyin_entries_reach_the_ranking_through_the_fallback_pass() {
        let entries = vec![hit("微信截图.png", Kind::Image, 3)];
        assert_eq!(rank_names(&entries, "weixin", None), vec!["微信截图.png"]);
        assert_eq!(rank_names(&entries, "wx", None), vec!["微信截图.png"]);
    }

    /// 真机扫盘烟测（默认 `#[ignore]`，只在手动验索引时跑）：
    ///   cargo test real_scan_smoke -- --ignored --nocapture
    /// 验证「根目录可解析 → 真扫出条目 → 原子落盘 → 读回可解析 → 搜得到」整条链路。
    /// 会自动往 <本测试 exe 目录>\temp\file-index-cache.json 落一份索引（约十几 MB）。
    #[test]
    #[ignore]
    fn real_scan_smoke() {
        let roots = scan_roots();
        println!("scan roots: {roots:?}");
        assert!(!roots.is_empty(), "至少要有 %USERPROFILE% 一个根");
        let started = SystemTime::now();
        scan_and_save();
        let st = status();
        println!(
            "indexed {} entries in {}ms (truncated={})",
            st.count,
            started.elapsed().map(|d| d.as_millis()).unwrap_or(0),
            st.truncated
        );
        assert!(st.count > 50, "扫出 {} 条，太少了，像是根目录没解析成功", st.count);
        let text = fs::read_to_string(cache_path()).expect("索引文件应已落盘");
        let back: PersistedIndex = serde_json::from_str(&text).expect("落盘 JSON 必须能读回");
        assert_eq!(back.files.len(), st.count);
        assert_eq!(back.version, INDEX_VERSION);
        println!("cache bytes = {}", text.len());
        // 搜索路径只读内存索引
        let hits = search("a", None, 5);
        assert!(hits.iter().all(|h| !h.path.is_empty()), "命中项必须带回完整路径");
        // 空查询 = 最近文件，按修改时间倒序
        let recent = search("", None, 5);
        assert!(recent.windows(2).all(|w| w[0].modified >= w[1].modified));
    }
}

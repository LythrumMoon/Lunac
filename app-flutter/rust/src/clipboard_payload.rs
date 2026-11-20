//! 剪贴板负载（唤出时）：文件 > 图片 > 文本。
//!
//! v1 的 `notify_window_shown()` 是「读文本 + 读 CF_HDROP 路径，拼成一段字符串」
//! 再交给前端判定；v2 把判定收敛到 Rust，一次返回结构化 JSON：
//!   · 有 CF_HDROP（复制了文件）→ `{"kind":"files","paths":[...]}`；
//!   · 否则有 CF_DIB / CF_DIBV5（截图 / 图片数据）→ 写成
//!     `<exe 根>\temp\clipboard-<时间戳>.bmp` → `{"kind":"image","path":"..."}`；
//!   · 否则回退到现有文本读取（`clipboard::text()`，2000 码元上限同口径）
//!     → `{"kind":"text","text":"..."}`；
//!   · 都没有 → `{"kind":"empty"}`。
//!
//! **只读**：只做 `OpenClipboard` + `GetClipboardData` + `GlobalLock`，绝不写系统剪贴板。
//! 任何一步失败（含 BMP 落盘失败）都降级为 `empty`，不 panic。

use serde_json::json;

/// 一次唤出读到的剪贴板负载。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Payload {
    Files(Vec<String>),
    Image(String),
    Text(String),
    Empty,
}

impl Payload {
    /// 转成 C ABI 约定的 JSON 字符串（键序固定，便于 Dart / 测试断言）。
    pub fn to_json(&self) -> String {
        match self {
            Payload::Files(paths) => json!({ "kind": "files", "paths": paths }).to_string(),
            Payload::Image(path) => json!({ "kind": "image", "path": path }).to_string(),
            Payload::Text(text) => json!({ "kind": "text", "text": text }).to_string(),
            Payload::Empty => json!({ "kind": "empty" }).to_string(),
        }
    }
}

/// 优先级判定：文件 > 图片 > 文本 > empty。
///
/// 纯函数，不触碰真实剪贴板 —— 单测用它覆盖「无文件 / 无图片 → empty」的降级口径。
pub fn resolve(files: Vec<String>, image: Option<String>, text: String) -> Payload {
    if !files.is_empty() {
        return Payload::Files(files);
    }
    if let Some(path) = image {
        if !path.is_empty() {
            return Payload::Image(path);
        }
    }
    if !text.is_empty() {
        return Payload::Text(text);
    }
    Payload::Empty
}

/// 读一次剪贴板并组装负载（文件优先，其次图片，最后文本）。
pub fn read() -> Payload {
    let files = crate::clipboard::file_paths();
    if !files.is_empty() {
        return resolve(files, None, String::new());
    }

    let image = read_image_to_bmp();
    if image.is_some() {
        return resolve(Vec::new(), image, String::new());
    }

    resolve(Vec::new(), None, crate::clipboard::text())
}

/// `lunac_clipboard_payload` 的实现：直接给 JSON 字符串。
pub fn read_json() -> String {
    read().to_json()
}

// ── 图片：CF_DIB → BMP ───────────────────────────────────────────

#[cfg(target_os = "windows")]
fn read_image_to_bmp() -> Option<String> {
    let dib = read_dib_bytes()?;
    let bmp = dib_to_bmp(&dib).ok()?;
    write_temp_bmp(&bmp)
}

#[cfg(not(target_os = "windows"))]
fn read_image_to_bmp() -> Option<String> {
    // 非 Windows 暂不接；按「没有图片」降级。
    None
}

#[cfg(target_os = "windows")]
mod win {
    pub const CF_DIB: u32 = 8;
    pub const CF_DIBV5: u32 = 17;

    #[link(name = "user32")]
    extern "system" {
        pub fn OpenClipboard(hwnd: isize) -> i32;
        pub fn CloseClipboard() -> i32;
        pub fn GetClipboardData(format: u32) -> isize;
    }

    #[link(name = "kernel32")]
    extern "system" {
        // 与 `clipboard.rs` 保持同一签名，避免 Windows 同符号重复声明告警；
        // 取字节时由调用方自行 `as *const u8`。
        pub fn GlobalLock(handle: isize) -> *const u16;
        pub fn GlobalUnlock(handle: isize) -> i32;
        pub fn GlobalSize(handle: isize) -> usize;
    }
}

/// 读 CF_DIBV5 / CF_DIB 的原始字节（优先 V5：带 alpha 与完整掩码）。
#[cfg(target_os = "windows")]
fn read_dib_bytes() -> Option<Vec<u8>> {
    use win::*;

    /// 无论从哪条分支返回，都保证剪贴板被关闭（泄漏锁会卡住其它程序的复制）。
    struct ClipboardGuard;
    impl Drop for ClipboardGuard {
        fn drop(&mut self) {
            unsafe {
                CloseClipboard();
            }
        }
    }

    unsafe {
        if OpenClipboard(0) == 0 {
            return None;
        }
        let _guard = ClipboardGuard;

        let v5 = GetClipboardData(CF_DIBV5);
        let handle = if v5 != 0 { v5 } else { GetClipboardData(CF_DIB) };
        if handle == 0 {
            return None;
        }

        // BITMAPINFOHEADER 至少 40 字节；GlobalSize 防止坏句柄喂出越界读。
        let size = GlobalSize(handle);
        if size < 40 {
            return None;
        }
        let ptr = GlobalLock(handle);
        if ptr.is_null() {
            return None;
        }
        let bytes = std::slice::from_raw_parts(ptr as *const u8, size).to_vec();
        GlobalUnlock(handle);
        Some(bytes)
    }
}

/// 「截屏识别」用：当前剪贴板图片的指纹（无图 = None）。
///
/// 只哈希 CF_DIB 字节、不落盘。截屏流程先取一次基线，再等指纹变化 ——
/// 这样「剪贴板里本来就有一张旧图」不会被当成刚截的那张（v1 的覆盖窗
/// 天然没有这个问题，v2 走系统截屏浮层就必须自己判）。
pub fn image_fingerprint() -> Option<u64> {
    #[cfg(target_os = "windows")]
    {
        let dib = read_dib_bytes()?;
        Some(fingerprint(&dib))
    }
    #[cfg(not(target_os = "windows"))]
    {
        None
    }
}

/// 把**当前**剪贴板图片落盘成 bmp（截屏完成后取图用）。无图 / 写失败 = None。
pub fn save_image_now() -> Option<String> {
    read_image_to_bmp()
}

/// FNV-1a（64 位）：只用于「是不是同一张图」的快速判定，不做安全用途。
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// 把 BMP 写到 `<exe 根>\temp\clipboard-<时间戳>.bmp`。
///
/// 写入失败返回 None（调用方降级为 empty，绝不让唤出崩溃）。
fn write_temp_bmp(bytes: &[u8]) -> Option<String> {
    let exe = std::env::current_exe().ok()?;
    let root = exe.parent()?;
    let dir = root.join("temp");
    std::fs::create_dir_all(&dir).ok()?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .ok()?
        .as_millis();
    let path = dir.join(format!("clipboard-{stamp}.bmp"));
    std::fs::write(&path, bytes).ok()?;
    Some(path.display().to_string())
}

// ── BMP 编码（纯函数，可单测）────────────────────────────────────

/// CF_DIB → BMP 文件字节（前插 14 字节 `BITMAPFILEHEADER`）。
///
/// CF_DIB 是「去掉 BMP 文件头」的完整 DIB：信息头 + 调色板 / 颜色掩码 + 像素。
/// 口径：
///   · `bfOffBits = 14 + biSize + 调色板 + 掩码`；调色板仅 ≤ 8 位（`biClrUsed`
///     为零时按 `1 << biBitCount` 项）；BI_BITFIELDS 的 12 字节掩码只在
///     `biSize == 40`（BITMAPINFOHEADER）时额外计入（V4/V5 头已含掩码）；
///   · 行按 **4 字节对齐**：`stride = ((width × bitCount + 31) / 32) × 4`；
///     若 DIB 只给了头（`biSizeImage = 0`）而像素不足，按 `stride × |height|`
///     补零到完整像素区，保证文件能被图片解码器打开；
///   · 行序**原样保留**：CF_DIB 的正高度就是自底向上（BMP 默认），负高度是
///     top-down（BMP 也允许），这里不翻转、也不改写 `biHeight`；
///   · BI_JPEG / BI_PNG 的负载是整段 JPEG/PNG 码流而不是裸位图，拒绝包装成 BMP。
pub fn dib_to_bmp(dib: &[u8]) -> Result<Vec<u8>, &'static str> {
    const FILE_HEADER: usize = 14;

    if dib.len() < 40 {
        return Err("DIB 头不足 40 字节");
    }
    let bi_size = u32_at(dib, 0).ok_or("缺少 biSize")? as usize;
    if bi_size < 40 || bi_size > dib.len() {
        return Err("biSize 非法");
    }

    let width = i32_at(dib, 4).ok_or("缺少 biWidth")?;
    let height = i32_at(dib, 8).ok_or("缺少 biHeight")?;
    if width == 0 || height == 0 {
        return Err("尺寸为 0");
    }

    let bit_count = u16_at(dib, 14).ok_or("缺少 biBitCount")?;
    let compression = u32_at(dib, 16).ok_or("缺少 biCompression")?;
    if !matches!(bit_count, 1 | 4 | 8 | 16 | 24 | 32) {
        return Err("不支持的位深");
    }
    // BI_RGB(0) / BI_RLE8(1) / BI_RLE4(2) / BI_BITFIELDS(3) / BI_ALPHABITFIELDS(6)
    // 才是可包装的位图；BI_JPEG(4) / BI_PNG(5) 是编码后的图片码流。
    if !matches!(compression, 0 | 1 | 2 | 3 | 6) {
        return Err("压缩格式不是裸位图");
    }

    // 像素起点：信息头 + 调色板 / 颜色掩码。
    let mut header_size = bi_size;
    if compression == 3 || compression == 6 {
        if bi_size == 40 {
            header_size = header_size.checked_add(12).ok_or("掩码越界")?;
        }
    } else if bit_count <= 8 {
        let clr_used = u32_at(dib, 32).unwrap_or(0) as usize;
        let max_colors = 1usize << bit_count;
        let colors = if clr_used > 0 && clr_used <= max_colors {
            clr_used
        } else {
            max_colors
        };
        header_size = header_size
            .checked_add(colors.checked_mul(4).ok_or("调色板越界")?)
            .ok_or("调色板越界")?;
    }
    if header_size > dib.len() {
        return Err("调色板 / 掩码越界");
    }

    let mut body = dib.to_vec();
    // 只有未压缩位图需要保证「行对齐 × 行数」的像素区完整；RLE 负载原样拷贝。
    if matches!(compression, 0 | 3 | 6) {
        let stride = row_stride(width.unsigned_abs() as usize, bit_count)?;
        let pixels_len = stride
            .checked_mul(height.unsigned_abs() as usize)
            .ok_or("尺寸过大")?;
        // 坏头可能声称一张 10 万 × 10 万 的图：超过 512MB 直接拒绝，别把内存打满。
        if pixels_len > 512 * 1024 * 1024 {
            return Err("尺寸过大");
        }
        let expected = header_size.checked_add(pixels_len).ok_or("尺寸过大")?;
        if body.len() < expected {
            body.resize(expected, 0);
        }
    }

    let pixel_offset = FILE_HEADER
        .checked_add(header_size)
        .ok_or("像素偏移越界")?;
    let file_size = FILE_HEADER.checked_add(body.len()).ok_or("文件过大")?;
    if pixel_offset > u32::MAX as usize || file_size > u32::MAX as usize {
        return Err("超过 BMP 4GB 上限");
    }

    let mut bmp = Vec::with_capacity(file_size);
    bmp.extend_from_slice(b"BM");
    bmp.extend_from_slice(&(file_size as u32).to_le_bytes());
    bmp.extend_from_slice(&0u32.to_le_bytes()); // bfReserved1 + bfReserved2
    bmp.extend_from_slice(&(pixel_offset as u32).to_le_bytes());
    bmp.extend_from_slice(&body);
    Ok(bmp)
}

/// 一行像素按 4 字节对齐后的字节数。
fn row_stride(width: usize, bit_count: u16) -> Result<usize, &'static str> {
    let bits = width
        .checked_mul(bit_count as usize)
        .ok_or("行宽溢出")?;
    Ok((bits + 31) / 32 * 4)
}

fn u16_at(bytes: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        bytes.get(offset..offset + 2)?.try_into().ok()?,
    ))
}

fn u32_at(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

fn i32_at(bytes: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        bytes.get(offset..offset + 4)?.try_into().ok()?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 造一个 40 字节 BITMAPINFOHEADER。
    fn dib_header(width: i32, height: i32, bit_count: u16, compression: u32) -> Vec<u8> {
        let mut header = vec![0u8; 40];
        header[0..4].copy_from_slice(&40u32.to_le_bytes()); // biSize
        header[4..8].copy_from_slice(&width.to_le_bytes()); // biWidth
        header[8..12].copy_from_slice(&height.to_le_bytes()); // biHeight
        header[12..14].copy_from_slice(&1u16.to_le_bytes()); // biPlanes
        header[14..16].copy_from_slice(&bit_count.to_le_bytes()); // biBitCount
        header[16..20].copy_from_slice(&compression.to_le_bytes()); // biCompression
        header
    }

    #[test]
    fn bmp_header_fields_offsets_and_row_order_match_the_dib() {
        // 3×2 / 24 位：每行 9 字节，按 4 字节对齐后是 12 字节。
        let mut dib = dib_header(3, 2, 24, 0);
        // 第一行 = 图像底行（正高度 = 自底向上）：9 字节像素 + 3 字节对齐填充。
        dib.extend_from_slice(&[1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0]);
        // 第二行 = 图像顶行。
        dib.extend_from_slice(&[10, 11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0]);

        let bmp = dib_to_bmp(&dib).expect("编码成功");

        // BITMAPFILEHEADER
        assert_eq!(&bmp[0..2], b"BM");
        assert_eq!(
            u32::from_le_bytes(bmp[2..6].try_into().unwrap()),
            bmp.len() as u32
        );
        assert_eq!(u32::from_le_bytes(bmp[6..10].try_into().unwrap()), 0);
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 54); // 14 + 40

        // BITMAPINFOHEADER 原样拷贝（文件偏移 14 起）
        assert_eq!(u32::from_le_bytes(bmp[14..18].try_into().unwrap()), 40);
        assert_eq!(i32::from_le_bytes(bmp[18..22].try_into().unwrap()), 3);
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), 2);
        assert_eq!(u16::from_le_bytes(bmp[28..30].try_into().unwrap()), 24);

        // 像素偏移 54；底行在前（行序未被翻转），每行含 3 字节对齐填充
        assert_eq!(&bmp[54..66], &[1, 2, 3, 4, 5, 6, 7, 8, 9, 0, 0, 0]);
        assert_eq!(
            &bmp[66..78],
            &[10, 11, 12, 13, 14, 15, 16, 17, 18, 0, 0, 0]
        );
    }

    #[test]
    fn missing_pixel_bytes_are_zero_padded_to_aligned_rows() {
        // 只给头（biSizeImage = 0）：按 3×2×24 位补 24 字节（含每行 3 字节对齐）。
        let dib = dib_header(3, 2, 24, 0);
        let bmp = dib_to_bmp(&dib).unwrap();
        assert_eq!(bmp.len(), 14 + 40 + 24);
        assert!(bmp[54..].iter().all(|b| *b == 0));
    }

    #[test]
    fn palette_and_bitfield_masks_are_included_in_the_pixel_offset() {
        // 8 位索引色：256 色调色板 = 1024 字节 → bfOffBits = 14 + 40 + 1024。
        let mut indexed = dib_header(2, 2, 8, 0);
        indexed.resize(40 + 1024 + 8, 0); // 每行 2 像素 = 4 字节（已验证对齐），共 2 行
        let bmp = dib_to_bmp(&indexed).unwrap();
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 1078);

        // 40 字节头 + BI_BITFIELDS：额外 12 字节掩码 → bfOffBits = 14 + 52。
        let mut bitfields = dib_header(1, 1, 32, 3);
        bitfields.extend_from_slice(&[0u8; 12]); // 三色掩码
        bitfields.extend_from_slice(&[1, 2, 3, 4]); // 1 像素
        let bmp = dib_to_bmp(&bitfields).unwrap();
        assert_eq!(u32::from_le_bytes(bmp[10..14].try_into().unwrap()), 66);
    }

    #[test]
    fn negative_height_is_kept_top_down_instead_of_being_flipped() {
        // 负高度 = top-down：像素与 biHeight 原样保留（BMP 允许）。
        let mut dib = dib_header(2, -1, 24, 0);
        dib.extend_from_slice(&[9, 8, 7, 6, 5, 4, 0, 0]);
        let bmp = dib_to_bmp(&dib).unwrap();
        assert_eq!(i32::from_le_bytes(bmp[22..26].try_into().unwrap()), -1);
        assert_eq!(&bmp[54..60], &[9, 8, 7, 6, 5, 4]);
    }

    #[test]
    fn encoded_jpeg_png_and_broken_headers_are_rejected() {
        let jpeg = dib_header(1, 1, 24, 4);
        assert!(dib_to_bmp(&jpeg).is_err());
        let png = dib_header(1, 1, 32, 5);
        assert!(dib_to_bmp(&png).is_err());
        assert!(dib_to_bmp(&[0u8; 10]).is_err());

        let mut bad_size = dib_header(1, 1, 24, 0);
        bad_size[0..4].copy_from_slice(&400u32.to_le_bytes());
        assert!(dib_to_bmp(&bad_size).is_err());

        let zero_size = dib_header(0, 1, 24, 0);
        assert!(dib_to_bmp(&zero_size).is_err());

        let bad_depth = dib_header(1, 1, 7, 0);
        assert!(dib_to_bmp(&bad_depth).is_err());
    }

    #[test]
    fn fingerprint_is_stable_and_changes_with_pixels() {
        // 同一份字节稳定；差一个像素就变（截屏流程靠它判断「换了新图」）。
        assert_eq!(fingerprint(&[1, 2, 3]), fingerprint(&[1, 2, 3]));
        assert_ne!(fingerprint(&[1, 2, 3]), fingerprint(&[1, 2, 4]));
        assert_ne!(fingerprint(&[]), fingerprint(&[0]));
    }

    #[test]
    fn payload_priority_is_files_then_image_then_text_then_empty() {
        assert_eq!(
            resolve(Vec::new(), None, String::new()),
            Payload::Empty
        );
        assert_eq!(
            resolve(Vec::new(), None, "hi".to_string()),
            Payload::Text("hi".to_string())
        );
        assert_eq!(
            resolve(
                Vec::new(),
                Some(r"D:\temp\clipboard-1.bmp".to_string()),
                "hi".to_string()
            ),
            Payload::Image(r"D:\temp\clipboard-1.bmp".to_string())
        );
        assert_eq!(
            resolve(
                vec![r"D:\Docs\a.pdf".to_string()],
                Some(r"D:\temp\clipboard-1.bmp".to_string()),
                "hi".to_string()
            ),
            Payload::Files(vec![r"D:\Docs\a.pdf".to_string()])
        );
        // 空图片路径不得抢占文本。
        assert_eq!(
            resolve(Vec::new(), Some(String::new()), "hi".to_string()),
            Payload::Text("hi".to_string())
        );
    }

    #[test]
    fn payload_json_uses_the_agreed_contract() {
        assert_eq!(Payload::Empty.to_json(), r#"{"kind":"empty"}"#);
        assert_eq!(
            Payload::Image("C:\\t\\a.bmp".to_string()).to_json(),
            r#"{"kind":"image","path":"C:\\t\\a.bmp"}"#
        );
        let files = Payload::Files(vec!["C:\\a.txt".to_string()]);
        assert_eq!(
            files.to_json(),
            r#"{"kind":"files","paths":["C:\\a.txt"]}"#
        );
    }
}

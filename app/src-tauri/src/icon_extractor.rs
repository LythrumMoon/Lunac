// ── Icon extractor: Windows GDI → base64 PNG ──────────────────────
// Uses SHGetFileInfoW to extract the system icon for any file,
// converts HICON→GDI bitmap→RGBA pixels→PNG (stored deflate, no deps).

#![allow(non_snake_case)]

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use base64::Engine as _;

// ── Windows GDI FFI ─────────────────────────────────────────────

const SHGFI_ICON: u32 = 0x100;
const SHGFI_LARGEICON: u32 = 0x000;

const FILE_ATTRIBUTE_NORMAL: u32 = 0x80;

const ICON_SIZE: i32 = 32;

#[repr(C)]
struct SHFILEINFOW {
    hIcon: isize,
    iIcon: i32,
    dwAttributes: u32,
    szDisplayName: [u16; 260],
    szTypeName: [u16; 80],
}

#[repr(C)]
struct BITMAPINFOHEADER {
    biSize: u32,
    biWidth: i32,
    biHeight: i32,
    biPlanes: u16,
    biBitCount: u16,
    biCompression: u32,
    biSizeImage: u32,
    biXPelsPerMeter: i32,
    biYPelsPerMeter: i32,
    biClrUsed: u32,
    biClrImportant: u32,
}

#[repr(C)]
struct ICONINFO {
    fIcon: i32,
    xHotspot: u32,
    yHotspot: u32,
    hbmMask: isize,
    hbmColor: isize,
}

const BI_RGB: u32 = 0;
const DIB_RGB_COLORS: u32 = 0;

#[link(name = "shell32")]
extern "system" {
    fn SHGetFileInfoW(
        pszPath: *const u16,
        dwFileAttributes: u32,
        psfi: *mut SHFILEINFOW,
        cbFileInfo: u32,
        uFlags: u32,
    ) -> usize;
}

#[link(name = "user32")]
extern "system" {
    fn GetIconInfo(hIcon: isize, piconinfo: *mut ICONINFO) -> i32;
    fn DestroyIcon(hIcon: isize) -> i32;
    fn GetDC(hWnd: isize) -> isize;
    fn ReleaseDC(hWnd: isize, hDC: isize) -> i32;
}

#[link(name = "gdi32")]
extern "system" {
    fn CreateCompatibleDC(hdc: isize) -> isize;
    fn DeleteDC(hdc: isize) -> i32;
    fn CreateDIBSection(
        hdc: isize,
        pbmi: *const BITMAPINFOHEADER,
        usage: u32,
        ppvBits: *mut *mut u8,
        hSection: isize,
        offset: u32,
    ) -> isize;
    fn SelectObject(hdc: isize, h: isize) -> isize;
    fn DeleteObject(hObject: isize) -> i32;
    fn DrawIconEx(
        hdc: isize,
        xLeft: i32,
        yTop: i32,
        hIcon: isize,
        cxWidth: i32,
        cyWidth: i32,
        istepIfAniCur: u32,
        hbrFlickerFreeDraw: isize,
        diFlags: u32,
    ) -> i32;
}

const IMAGE_ICON: u32 = 1;
const LR_LOADFROMFILE: u32 = 0x0010;

#[link(name = "user32")]
extern "system" {
    fn LoadImageW(
        hInst: isize,
        name: *const u16,
        r#type: u32,
        cx: i32,
        cy: i32,
        fuLoad: u32,
    ) -> isize;
}

// ── PNG encoding (stored deflate, zero deps) ───────────────────

/// Adler-32 checksum for zlib.
fn adler32(data: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in data {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

/// CRC-32 for PNG chunks. Table-driven.
fn crc32(data: &[u8]) -> u32 {
    static CRC_TABLE: [u32; 256] = {
        let mut table = [0u32; 256];
        let mut n = 0u32;
        while n < 256 {
            let mut c = n;
            let mut _k = 0;
            while _k < 8 {
                if c & 1 != 0 {
                    c = 0xEDB88320 ^ (c >> 1);
                } else {
                    c >>= 1;
                }
                _k += 1;
            }
            table[n as usize] = c;
            n += 1;
        }
        table
    };

    let mut crc: u32 = 0xFFFFFFFF;
    for byte in data {
        crc = CRC_TABLE[((crc ^ *byte as u32) & 0xFF) as usize] ^ (crc >> 8);
    }
    crc ^ 0xFFFFFFFF
}

fn png_chunk(chunk_type: &[u8; 4], data: &[u8]) -> Vec<u8> {
    let len = data.len() as u32;
    let mut buf = Vec::with_capacity(12 + data.len());
    buf.extend_from_slice(&len.to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(chunk_type);
    crc_input.extend_from_slice(data);
    buf.extend_from_slice(chunk_type);
    buf.extend_from_slice(data);
    buf.extend_from_slice(&crc32(&crc_input).to_be_bytes());
    buf
}

/// Encode raw RGBA pixel data (top-down scanlines) as PNG.
fn encode_png_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    assert_eq!(pixels.len(), (width * height * 4) as usize);

    // Build raw scanlines with filter byte (0 = None)
    let row_stride = (width * 4) as usize;
    let mut raw: Vec<u8> = Vec::with_capacity((height as usize) * (row_stride + 1));
    for y in 0..height as usize {
        raw.push(0); // filter: None
        let start = y * row_stride;
        raw.extend_from_slice(&pixels[start..start + row_stride]);
    }

    // Deflate: stored blocks
    let mut deflate = Vec::new();
    // BFINAL=1, BTYPE=00 (stored)
    deflate.push(1);
    let len = raw.len() as u16;
    deflate.extend_from_slice(&len.to_le_bytes());
    deflate.extend_from_slice(&(!len).to_le_bytes());
    deflate.extend_from_slice(&raw);

    // zlib wrapper
    let cmf: u8 = 0x78; // deflate, window=32K
    let flg: u8 = 0x01; // check bits
    let adler = adler32(&raw);

    let mut zlib = Vec::new();
    zlib.push(cmf);
    zlib.push(flg);
    zlib.extend_from_slice(&deflate);
    zlib.extend_from_slice(&adler.to_be_bytes());

    // PNG signature
    let mut png: Vec<u8> = vec![137, 80, 78, 71, 13, 10, 26, 10];

    // IHDR
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // color type: RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));

    // IDAT
    png.extend_from_slice(&png_chunk(b"IDAT", &zlib));

    // IEND
    png.extend_from_slice(&png_chunk(b"IEND", &[]));

    png
}

// ── Icon extraction ─────────────────────────────────────────────

/// Convert Windows wide string path to UTF-16 for FFI.
fn to_wide(s: &str) -> Vec<u16> {
    OsStr::new(s).encode_wide().chain(std::iter::once(0)).collect()
}

/// 图标抽取的**进程内串行闸**。
///
/// Windows 这条链（`SHGetFileInfoW` 的主图标缓存路径 + `GetDC(0)` / `CreateDIBSection` /
/// `DrawIconEx` 的 GDI 设备上下文）**并发调用会互相踩**，而踩出来的结果是「拿不到图标」——
/// 不是报错，是静默返回 `None`（界面上表现为某几行退回文字占位符）。
///
/// 实测（2026-09-22，新进程 + CDP 探针）：同一批 6 个 exe 并发请求，**第一个**批只回来 1 个
/// （`010000`），此后同样的批跑 4 次全是 `111111`；而改之前（`get_app_icon` 是同步命令，
/// 跑在主线程上天然串行）同样的新进程第一批就是 `111111`。所以：**并发是根因，且这是搬出
/// 主线程后新引入的**（搬出主线程本身是对的，见 `commands::run_blocking`）。
///
/// 收口方式：本函数是**唯一**的图标抽取入口（`extract_thumbnail_base64` 的回落也走它），
/// 在这里串行就同时管住了 `get_app_icon` 与 `get_file_thumbnail` 两条路 —— 并发请求变成
/// 在阻塞线程池里排队，主线程照样不被占（这才是我们要的），结果恢复确定性。
static ICON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Extract a 32x32 icon from a file path, return as base64 PNG data URL.
/// Only works for .exe, .lnk, .dll, .ico files etc.
pub fn extract_icon_base64(path: &str) -> Option<String> {
    // 中毒也继续用（上一次抽取 panic 不该让此后所有图标都拿不到）
    let _serial = ICON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    unsafe {
        let wide = to_wide(path);

        // Try SHGetFileInfoW first — works for all file types using system icon cache
        let mut sfi: SHFILEINFOW = std::mem::zeroed();
        let ret = SHGetFileInfoW(
            wide.as_ptr(),
            FILE_ATTRIBUTE_NORMAL,
            &mut sfi,
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ret == 0 || sfi.hIcon == 0 {
            // Fallback: try LoadImage for .exe/.ico files
            let hicon = LoadImageW(
                0,
                wide.as_ptr(),
                IMAGE_ICON,
                ICON_SIZE,
                ICON_SIZE,
                LR_LOADFROMFILE,
            );
            if hicon == 0 {
                return None;
            }
            let png = icon_to_png(hicon);
            DestroyIcon(hicon);
            return png;
        }

        let png = icon_to_png(sfi.hIcon);
        DestroyIcon(sfi.hIcon);
        png
    }
}

unsafe fn icon_to_png(hicon: isize) -> Option<String> {
    let mut ii: ICONINFO = std::mem::zeroed();
    if GetIconInfo(hicon, &mut ii) == 0 {
        return None;
    }

    let hdc = GetDC(0);
    if hdc == 0 {
        cleanup_icon_info(&ii);
        return None;
    }

    let mem_dc = CreateCompatibleDC(hdc);
    if mem_dc == 0 {
        ReleaseDC(0, hdc);
        cleanup_icon_info(&ii);
        return None;
    }

    // Create a 32x32 ARGB DIB section
    let mut bi: BITMAPINFOHEADER = std::mem::zeroed();
    bi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    bi.biWidth = ICON_SIZE;
    bi.biHeight = -ICON_SIZE; // top-down
    bi.biPlanes = 1;
    bi.biBitCount = 32;
    bi.biCompression = BI_RGB;

    let mut pv_bits: *mut u8 = std::ptr::null_mut();
    let dib = CreateDIBSection(
        mem_dc,
        &bi,
        DIB_RGB_COLORS,
        &mut pv_bits,
        0,
        0,
    );
    if dib == 0 || pv_bits.is_null() {
        DeleteDC(mem_dc);
        ReleaseDC(0, hdc);
        cleanup_icon_info(&ii);
        return None;
    }

    let old_bmp = SelectObject(mem_dc, dib);

    // Fill with transparent black (NULL_BRUSH = no fill, background stays 0)
    let _old_brush = SelectObject(mem_dc, 0);

    // Draw icon centered
    DrawIconEx(mem_dc, 0, 0, hicon, ICON_SIZE, ICON_SIZE, 0, 0, 0x0003); // DI_NORMAL | DI_COMPAT

    // Read pixels — fill background with transparent black first
    let byte_count = (ICON_SIZE * ICON_SIZE * 4) as usize;
    // Actually the DIB section pixels are already filled by DrawIconEx.
    // Read them into a Vec.
    let mut pixels: Vec<u8> = Vec::with_capacity(byte_count);
    pixels.set_len(byte_count);
    std::ptr::copy_nonoverlapping(pv_bits, pixels.as_mut_ptr(), byte_count);

    // BGRA → RGBA (Windows DIB uses BGRA byte order)
    for chunk in pixels.chunks_exact_mut(4) {
        // chunk = [B, G, R, A]
        chunk.swap(0, 2); // swap B and R
    }

    // Clean up GDI
    SelectObject(mem_dc, old_bmp);
    DeleteObject(dib);
    DeleteDC(mem_dc);
    ReleaseDC(0, hdc);
    cleanup_icon_info(&ii);

    // Encode as PNG
    let png = encode_png_rgba(ICON_SIZE as u32, ICON_SIZE as u32, &pixels);
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Some(format!("data:image/png;base64,{}", b64))
}

unsafe fn cleanup_icon_info(ii: &ICONINFO) {
    if ii.hbmColor != 0 {
        DeleteObject(ii.hbmColor);
    }
    if ii.hbmMask != 0 {
        DeleteObject(ii.hbmMask);
    }
}

// ── 文件缩略图（详细搜索右侧预览区，2026-09-19 批 5 任务 3a）──────────

/// 能**真解码**出缩略图的扩展名白名单。
/// 只放 `image` crate 实际开了 feature 的格式（Cargo.toml 里是 png + jpeg）——
/// gif / webp / bmp / tiff 走系统类型图标，别在这里假装能缩略（会得到一张
/// 「解码失败」的空图，比直接给类型图标更糟）。
const THUMBNAIL_EXTS: &[&str] = &["png", "jpg", "jpeg", "jfif"];

/// 解码体积上限。预览区是「顺手看一眼」：为一个 40MB 的 PNG 解码 + 缩放会把
/// 详细搜索的重渲染卡住（该命令虽是 `async`，但内存峰值是实打实的）。超限回落类型图标。
const THUMBNAIL_MAX_BYTES: u64 = 24 * 1024 * 1024;

/// 文件缩略图 → base64 PNG data URL。
///
/// 图片（`THUMBNAIL_EXTS`）真解码并缩到长边 `max`；**其余一切情况**
/// （非图片 / 解码失败 / 文件过大 / 路径不存在）回落 [`extract_icon_base64`]
/// 的系统类型图标。所以调用方永远只面对「有图 / 没图」两种结果，
/// 不必自己兜底 —— 预览区拿到 `None` 就画占位符即可。
pub fn extract_thumbnail_base64(path: &str, max: u32) -> Option<String> {
    if is_thumbnail_candidate(path) {
        if let Some(url) = decode_thumbnail_base64(path, max) {
            return Some(url);
        }
    }
    extract_icon_base64(path)
}

fn is_thumbnail_candidate(path: &str) -> bool {
    let ext = Path::new(path)
        .extension()
        .and_then(OsStr::to_str)
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_default();
    THUMBNAIL_EXTS.contains(&ext.as_str())
}

fn decode_thumbnail_base64(path: &str, max: u32) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() > THUMBNAIL_MAX_BYTES {
        return None;
    }
    let img = image::open(path).ok()?;
    // `thumbnail` 保持纵横比、只缩不放（比 max 小的图原样返回）
    let thumb = img.thumbnail(max.clamp(16, 512), max.clamp(16, 512));
    let mut png: Vec<u8> = Vec::new();
    thumb
        .write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    Some(format!("data:image/png;base64,{}", b64))
}

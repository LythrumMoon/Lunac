//! 剪贴板读取（v2）：唤出时把文本 / 文件带进搜索栏的底层能力。
//!
//! v1 在 `notify_window_shown()` 里用 `arboard` 读文本、再用 CF_HDROP 取图片路径，
//! 通过事件发给前端。v2 的窗口与事件都在 Dart，所以这里只提供**同步读取**的 C ABI：
//!   · `text()`：CF_UNICODETEXT（唤出时填进搜索框）；
//!   · `file_paths()`：CF_HDROP 的全部路径（v1 的 `read_clipboard_file_paths`，粘贴用）；
//!   · `image_files()`：CF_HDROP 里扩展名是图片的路径（v1 的 `read_clipboard_files`，
//!     唤出时判断「剪贴板里有图片」并插入 OCR 项用）。
//!
//! 仍留在 Rust 的原因：Dart 的 `Clipboard.getData` 拿不到 CF_HDROP / CF_DIB。
//! 口径与 v1 一致：超过 2000 字符的文本不采用（避免把整篇文档灌进搜索框）。

#[cfg(target_os = "windows")]
mod win {
    pub const CF_UNICODETEXT: u32 = 13;
    pub const CF_HDROP: u32 = 15;
    /// DragQueryFileW 的「取文件数」特殊下标。
    pub const DRAG_FILE_COUNT: u32 = 0xFFFF_FFFF;

    #[link(name = "user32")]
    extern "system" {
        pub fn OpenClipboard(hwnd: isize) -> i32;
        pub fn CloseClipboard() -> i32;
        pub fn GetClipboardData(format: u32) -> isize;
        pub fn IsClipboardFormatAvailable(format: u32) -> i32;
    }

    #[link(name = "kernel32")]
    extern "system" {
        pub fn GlobalLock(handle: isize) -> *const u16;
        pub fn GlobalUnlock(handle: isize) -> i32;
    }

    #[link(name = "shell32")]
    extern "system" {
        pub fn DragQueryFileW(hdrop: isize, index: u32, buf: *mut u16, size: u32) -> u32;
    }
}

#[cfg(target_os = "windows")]
pub fn text() -> String {
    use win::*;

    let mut out = String::new();
    unsafe {
        if OpenClipboard(0) == 0 {
            return out;
        }
        let handle = GetClipboardData(CF_UNICODETEXT);
        if handle != 0 {
            let ptr = GlobalLock(handle);
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                out = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
                GlobalUnlock(handle);
            }
        }
        CloseClipboard();
    }

    let trimmed = out.trim();
    if trimmed.chars().count() > 2000 {
        return String::new();
    }
    trimmed.to_string()
}

#[cfg(not(target_os = "windows"))]
pub fn text() -> String {
    // Linux 的剪贴板读取（xclip / wl-paste）留到 Linux 补验时接。
    String::new()
}

/// CF_HDROP 里的全部路径（复制文件后粘贴 / 拖拽的同口径来源）。
#[cfg(target_os = "windows")]
pub fn file_paths() -> Vec<String> {
    use win::*;

    let mut out = Vec::new();
    unsafe {
        if OpenClipboard(0) == 0 {
            return out;
        }
        if IsClipboardFormatAvailable(CF_HDROP) != 0 {
            let handle = GetClipboardData(CF_HDROP);
            if handle != 0 {
                let count = DragQueryFileW(handle, DRAG_FILE_COUNT, std::ptr::null_mut(), 0);
                for index in 0..count {
                    // 先问长度（不含 NUL），再按长度 +1 取内容。
                    let len = DragQueryFileW(handle, index, std::ptr::null_mut(), 0) as usize;
                    if len == 0 {
                        continue;
                    }
                    let mut buffer: Vec<u16> = vec![0; len + 1];
                    let written =
                        DragQueryFileW(handle, index, buffer.as_mut_ptr(), (len + 1) as u32);
                    if written > 0 {
                        buffer.truncate(written as usize);
                        out.push(String::from_utf16_lossy(&buffer));
                    }
                }
            }
        }
        CloseClipboard();
    }
    out
}

#[cfg(not(target_os = "windows"))]
pub fn file_paths() -> Vec<String> {
    Vec::new()
}

/// CF_HDROP 里扩展名是图片的路径（v1：唤出时判断剪贴板是否有图片，用于 OCR 入口）。
pub fn image_files() -> Vec<String> {
    file_paths()
        .into_iter()
        .filter(|path| is_image_path(path))
        .collect()
}

/// 图片扩展名白名单（与 v1 `read_clipboard_files` 一致）。
const IMAGE_EXTS: &[&str] = &["png", "jpg", "jpeg", "bmp", "tiff", "webp", "gif", "ico"];

fn is_image_path(path: &str) -> bool {
    std::path::Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| {
            let lower = ext.to_ascii_lowercase();
            IMAGE_EXTS.contains(&lower.as_str())
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_image_extensions_are_detected_case_insensitively() {
        assert!(is_image_path(r"D:\pics\a.PNG"));
        assert!(is_image_path("C:/x/y.jpeg"));
        assert!(!is_image_path(r"D:\docs\a.pdf"));
        assert!(!is_image_path("no_extension"));
        assert!(!is_image_path(""));
    }
}

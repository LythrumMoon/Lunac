// src-tauri/src/windows_ocr.rs
// Windows 10/11 内置 OCR API (Windows.Media.Ocr.OcrEngine)
// 零 CDN 依赖、原生速度、支持 zh-Hans 中文识别。
//
// 仅在 Windows 平台编译，其他平台返回错误。

use std::path::Path;

#[cfg(target_os = "windows")]
pub fn recognize_image(path: &str) -> Result<String, String> {
    // Import Windows APIs — done inside the function so they don't
    // pollute module-level scope when compiling on non-Windows.
    use windows::core::HSTRING;
    use windows::Graphics::Imaging::BitmapDecoder;
    use windows::Media::Ocr::OcrEngine;
    use windows::Storage::{FileAccessMode, StorageFile};

    let file_path = Path::new(path);
    if !file_path.exists() {
        return Err(format!("File not found: {}", path));
    }

    // Load image file via Windows Runtime StorageFile
    let hpath = HSTRING::from(path);
    let storage_file = StorageFile::GetFileFromPathAsync(&hpath)
        .map_err(|e| format!("StorageFile error: {e}"))?
        .get()
        .map_err(|e| format!("GetFileFromPathAsync failed: {e}"))?;

    // Open file stream
    let stream = storage_file
        .OpenAsync(FileAccessMode::Read)
        .map_err(|e| format!("OpenAsync error: {e}"))?
        .get()
        .map_err(|e| format!("OpenAsync failed: {e}"))?;

    // Decode bitmap
    let decoder = BitmapDecoder::CreateAsync(&stream)
        .map_err(|e| format!("BitmapDecoder error: {e}"))?
        .get()
        .map_err(|e| format!("BitmapDecoder failed: {e}"))?;

    // Get the first frame (pixel data)
    let bitmap_frame = decoder
        .GetFrameAsync(0)
        .map_err(|e| format!("GetFrameAsync error: {e}"))?
        .get()
        .map_err(|e| format!("GetFrameAsync failed: {e}"))?;

    // Convert to SoftwareBitmap (BGRA8 required for OCR)
    let software_bitmap = bitmap_frame
        .GetSoftwareBitmapAsync()
        .map_err(|e| format!("GetSoftwareBitmapAsync error: {e}"))?
        .get()
        .map_err(|e| format!("GetSoftwareBitmapAsync failed: {e}"))?;

    // Create OCR engine using user's system language profile.
    // UWP OCR supports all languages installed in Windows Settings → Language.
    // For Chinese: install Chinese language pack in Windows Settings.
    let engine = OcrEngine::TryCreateFromUserProfileLanguages()
        .map_err(|e| format!("OcrEngine creation failed: {e}. Install language pack in Windows Settings."))?;

    let result = engine
        .RecognizeAsync(&software_bitmap)
        .map_err(|e| format!("RecognizeAsync error: {e}"))?
        .get()
        .map_err(|e| format!("RecognizeAsync failed: {e}"))?;

    let text = result.Text().map_err(|e| format!("Text extraction failed: {e}"))?;
    Ok(text.to_string())
}

#[cfg(not(target_os = "windows"))]
pub fn recognize_image(_path: &str) -> Result<String, String> {
    Err("Windows OCR is only available on Windows 10+".into())
}

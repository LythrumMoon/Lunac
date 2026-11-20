//! Windows 应用图标抽取（32×32 → PNG → base64 data URL）。
//!
//! 从 v1 `app/src-tauri/src/icon_extractor.rs` 抽出（只搬主图标这一条；
//! 详细搜索用的文件缩略图 `extract_thumbnail_base64` 留在 v1，M6 再统一）。
//!
//! 为什么放在共享 crate：v2 的搜索结果行要显示真实应用图标，而图标抽取是
//! `SHGetFileInfoW` + GDI 的裸调用，不应在 Dart 侧重写；v1 / v2 用同一份实现。
//!
//! **串行闸**：这条链并发调用会静默拿不到图标（实测见 v1 注释），所以这里用一把
//! 进程内 Mutex 串行化所有抽取请求。
#![allow(non_snake_case)]

#[cfg(not(target_os = "windows"))]
pub fn extract_icon_base64(_path: &str) -> Option<String> {
    // Linux 的图标抽取（.desktop / 主题图标）留到 Linux 补验时接。
    None
}

#[cfg(target_os = "windows")]
pub use windows_impl::extract_icon_base64;

#[cfg(target_os = "windows")]
mod windows_impl {
    use base64::Engine as _;
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

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
    const IMAGE_ICON: u32 = 1;
    const LR_LOADFROMFILE: u32 = 0x0010;

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
        fn LoadImageW(
            hInst: isize,
            name: *const u16,
            r#type: u32,
            cx: i32,
            cy: i32,
            fuLoad: u32,
        ) -> isize;
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

    static ICON_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// 从文件路径抽 32×32 图标，返回 `data:image/png;base64,...`。
    pub fn extract_icon_base64(path: &str) -> Option<String> {
        // 中毒也继续用（一次 panic 不该让此后所有图标都拿不到）
        let _serial = ICON_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        unsafe {
            let wide = to_wide(path);

            let mut sfi: SHFILEINFOW = std::mem::zeroed();
            let ret = SHGetFileInfoW(
                wide.as_ptr(),
                FILE_ATTRIBUTE_NORMAL,
                &mut sfi,
                std::mem::size_of::<SHFILEINFOW>() as u32,
                SHGFI_ICON | SHGFI_LARGEICON,
            );
            if ret == 0 || sfi.hIcon == 0 {
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

    fn to_wide(s: &str) -> Vec<u16> {
        OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
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

        let mut bi: BITMAPINFOHEADER = std::mem::zeroed();
        bi.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bi.biWidth = ICON_SIZE;
        bi.biHeight = -ICON_SIZE; // top-down
        bi.biPlanes = 1;
        bi.biBitCount = 32;
        bi.biCompression = BI_RGB;

        let mut pv_bits: *mut u8 = std::ptr::null_mut();
        let dib = CreateDIBSection(mem_dc, &bi, DIB_RGB_COLORS, &mut pv_bits, 0, 0);
        if dib == 0 || pv_bits.is_null() {
            DeleteDC(mem_dc);
            ReleaseDC(0, hdc);
            cleanup_icon_info(&ii);
            return None;
        }

        let old_bmp = SelectObject(mem_dc, dib);
        let _old_brush = SelectObject(mem_dc, 0);
        // DI_NORMAL | DI_COMPAT
        DrawIconEx(mem_dc, 0, 0, hicon, ICON_SIZE, ICON_SIZE, 0, 0, 0x0003);

        let byte_count = (ICON_SIZE * ICON_SIZE * 4) as usize;
        let mut pixels: Vec<u8> = Vec::with_capacity(byte_count);
        pixels.set_len(byte_count);
        std::ptr::copy_nonoverlapping(pv_bits, pixels.as_mut_ptr(), byte_count);

        // Windows DIB 是 BGRA；PNG 要 RGBA。
        for chunk in pixels.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }

        SelectObject(mem_dc, old_bmp);
        DeleteObject(dib);
        DeleteDC(mem_dc);
        ReleaseDC(0, hdc);
        cleanup_icon_info(&ii);

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

    // ── PNG 编码（stored deflate，零依赖）──────────────────────────

    fn adler32(data: &[u8]) -> u32 {
        let mut a: u32 = 1;
        let mut b: u32 = 0;
        for byte in data {
            a = (a + *byte as u32) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    fn crc32(data: &[u8]) -> u32 {
        static CRC_TABLE: [u32; 256] = {
            let mut table = [0u32; 256];
            let mut n = 0u32;
            while n < 256 {
                let mut c = n;
                let mut k = 0;
                while k < 8 {
                    if c & 1 != 0 {
                        c = 0xEDB88320 ^ (c >> 1);
                    } else {
                        c >>= 1;
                    }
                    k += 1;
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

    fn encode_png_rgba(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
        assert_eq!(pixels.len(), (width * height * 4) as usize);

        let row_stride = (width * 4) as usize;
        let mut raw: Vec<u8> = Vec::with_capacity((height as usize) * (row_stride + 1));
        for y in 0..height as usize {
            raw.push(0); // filter: None
            let start = y * row_stride;
            raw.extend_from_slice(&pixels[start..start + row_stride]);
        }

        // Deflate: 单块 stored
        let mut deflate = Vec::new();
        deflate.push(1); // BFINAL=1, BTYPE=00
        let len = raw.len() as u16;
        deflate.extend_from_slice(&len.to_le_bytes());
        deflate.extend_from_slice(&(!len).to_le_bytes());
        deflate.extend_from_slice(&raw);

        let mut zlib = Vec::new();
        zlib.push(0x78); // deflate, window=32K
        zlib.push(0x01); // check bits
        zlib.extend_from_slice(&deflate);
        zlib.extend_from_slice(&adler32(&raw).to_be_bytes());

        let mut png: Vec<u8> = vec![137, 80, 78, 71, 13, 10, 26, 10];

        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.push(8); // bit depth
        ihdr.push(6); // color type: RGBA
        ihdr.push(0);
        ihdr.push(0);
        ihdr.push(0);
        png.extend_from_slice(&png_chunk(b"IHDR", &ihdr));
        png.extend_from_slice(&png_chunk(b"IDAT", &zlib));
        png.extend_from_slice(&png_chunk(b"IEND", &[]));
        png
    }
}

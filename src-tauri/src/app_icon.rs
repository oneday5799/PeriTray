use image::RgbaImage;
use image::codecs::png::PngEncoder;
use image::ImageEncoder;
use lru::LruCache;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

static ICON_CACHE: OnceLock<Mutex<LruCache<u32, Arc<str>>>> = OnceLock::new();

/// 从进程PID获取应用图标（返回base64编码的PNG）
pub fn get_app_icon_by_pid(pid: u32) -> Option<Arc<str>> {
    let cache = ICON_CACHE.get_or_init(|| {
        Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap()))
    });
    {
        let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(icon) = guard.get(&pid) {
            return Some(Arc::clone(icon));
        }
    }
    let icon: Option<Arc<str>> = (|| -> Option<Arc<str>> {
        unsafe {
            let process_handle = windows::Win32::System::Threading::OpenProcess(
                windows::Win32::System::Threading::PROCESS_QUERY_INFORMATION | windows::Win32::System::Threading::PROCESS_VM_READ,
                false,
                pid,
            ).ok()?;
            let mut path_buf = [0u16; 260];
            let mut path_size = path_buf.len() as u32;
            let result = windows::Win32::System::Threading::QueryFullProcessImageNameW(
                process_handle,
                windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                windows::core::PWSTR(path_buf.as_mut_ptr()),
                &mut path_size,
            );
            let _ = windows::Win32::Foundation::CloseHandle(process_handle);
            if result.is_err() { return None; }
            let exe_path = String::from_utf16_lossy(&path_buf[..path_size as usize]);
            get_icon_from_path(&exe_path)
        }
    })();
    icon.as_ref()?;
    let mut guard = cache.lock().unwrap_or_else(|e| e.into_inner());
    guard.put(pid, Arc::clone(icon.as_ref().unwrap()));
    icon
}

/// 从文件路径提取图标（返回base64编码的PNG）
fn get_icon_from_path(path: &str) -> Option<Arc<str>> {
    unsafe {
        let mut path_buf = [0u16; 260];
        let path_wide: Vec<u16> = path.encode_utf16().chain(std::iter::once(0)).collect();
        let copy_len = path_wide.len().min(259);
        path_buf[..copy_len].copy_from_slice(&path_wide[..copy_len]);

        // 使用 PrivateExtractIconsW 获取图标
        let mut icons = [windows::Win32::UI::WindowsAndMessaging::HICON(std::ptr::null_mut()); 1];
        let icon_count = windows::Win32::UI::WindowsAndMessaging::PrivateExtractIconsW(
            &path_buf,
            0,
            64,
            64,
            Some(&mut icons),
            None,
            0,
        );

        if icon_count == 0 || icons[0].is_invalid() {
            return None;
        }

        // 将图标转换为位图
        let icon_info = get_icon_bitmap(icons[0])?;
        let _ = windows::Win32::UI::WindowsAndMessaging::DestroyIcon(icons[0]);

        // 转换为PNG base64
        bitmap_to_base64(&icon_info)
    }
}

/// 获取图标位图数据
unsafe fn get_icon_bitmap(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<RgbaImage> {
    use windows::Win32::Graphics::Gdi::*;

    let width = 64i32;
    let height = 64i32;

    // 创建设备上下文
    let hdc_screen = GetDC(Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())));
    if hdc_screen.is_invalid() {
        return None;
    }

    // 创建兼容的内存DC
    let hdc_mem = CreateCompatibleDC(Some(hdc_screen));
    if hdc_mem.is_invalid() {
        ReleaseDC(Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())), hdc_screen);
        return None;
    }

    // 创建DIB位图
    let mut bi = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width,
            biHeight: -height, // 自上而下
            biPlanes: 1,
            biBitCount: 32,
            biCompression: 0, // BI_RGB
            ..std::mem::zeroed()
        },
        bmiColors: [RGBQUAD::default(); 1],
    };

    let mut pixels = vec![0u8; (width * height * 4) as usize];

    // 创建DIB位图并选入DC
    let hbitmap = CreateDIBSection(
        Some(hdc_mem),
        &bi,
        DIB_RGB_COLORS,
        pixels.as_mut_ptr() as *mut _,
        None,
        0,
    ).ok()?;
    if hbitmap.is_invalid() {
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())), hdc_screen);
        return None;
    }

    // 选入DC
    let old_bitmap = SelectObject(hdc_mem, HGDIOBJ(hbitmap.0));

    // 绘制图标到DC
    let _ = windows::Win32::UI::WindowsAndMessaging::DrawIconEx(
        hdc_mem,
        0,
        0,
        hicon,
        width,
        height,
        0,
        None,
        windows::Win32::UI::WindowsAndMessaging::DI_NORMAL,
    );

    // 获取位图数据
    let bits = GetDIBits(
        hdc_mem,
        HBITMAP(hbitmap.0),
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as *mut _),
        &mut bi,
        DIB_RGB_COLORS,
    );

    // 清理资源
    SelectObject(hdc_mem, old_bitmap);
    let _ = DeleteObject(HGDIOBJ(hbitmap.0));
    let _ = DeleteDC(hdc_mem);
    let _ = ReleaseDC(Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())), hdc_screen);

    if bits == 0 {
        return None;
    }

    // 原地转换BGRA到RGBA（避免第二次堆分配）
    for chunk in pixels.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }

    RgbaImage::from_raw(width as u32, height as u32, pixels)
}

/// 将RGBA图像转换为base64编码的PNG
fn bitmap_to_base64(img: &RgbaImage) -> Option<Arc<str>> {
    let mut buffer = Cursor::new(Vec::with_capacity(16384));
    let encoder = PngEncoder::new(&mut buffer);
    encoder.write_image(
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    ).ok()?;

    use base64::Engine;
    Some(base64::engine::general_purpose::STANDARD.encode(buffer.into_inner()).into())
}

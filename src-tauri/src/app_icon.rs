use image::codecs::png::PngEncoder;
use image::ImageEncoder;
use image::RgbaImage;
use lru::LruCache;
use std::io::Cursor;
use std::num::NonZeroUsize;
use std::sync::{Arc, Mutex, OnceLock};

static ICON_CACHE: OnceLock<Mutex<LruCache<u32, Arc<str>>>> = OnceLock::new();
static NAME_CACHE: OnceLock<Mutex<LruCache<u32, Arc<str>>>> = OnceLock::new();

/// 从进程PID获取应用名称（优先读取 exe 文件版本信息的 FileDescription，回退到 exe 文件名）
pub fn get_process_name_by_pid(pid: u32) -> Option<Arc<str>> {
    let cache =
        NAME_CACHE.get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap())));
    {
        let mut guard = crate::state::lock_unpoisoned(cache);
        if let Some(name) = guard.get(&pid) {
            return Some(Arc::clone(name));
        }
    }
    let name: Option<Arc<str>> = resolve_process_name(pid).map(|s| Arc::from(s.as_str()));
    if let Some(name) = &name {
        let mut guard = crate::state::lock_unpoisoned(cache);
        guard.put(pid, Arc::clone(name));
    }
    name
}

/// 主路径：低权限 OpenProcess 查询 exe 路径（长路径安全）。
/// 取图标/名字仅需路径，无需读进程内存，故用 PROCESS_QUERY_LIMITED_INFORMATION，
/// 避免管理员进程因无 VM_READ 权限被拒。
fn query_exe_path_by_openprocess(pid: u32) -> Option<String> {
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_FORMAT,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };

    unsafe {
        let process_handle = match OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
            Ok(h) => h,
            Err(_) => {
                crate::process::append_verbose_log(&format!(
                    "[app_icon] OpenProcess pid={pid} 失败（权限/保护进程）"
                ));
                return None;
            }
        };
        let mut path_buf = [0u16; 32768];
        let mut path_size = path_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process_handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(path_buf.as_mut_ptr()),
            &mut path_size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(process_handle);
        if result.is_err() {
            crate::process::append_verbose_log(&format!(
                "[app_icon] QueryFullProcessImageNameW pid={pid} 失败"
            ));
            return None;
        }
        Some(String::from_utf16_lossy(&path_buf[..path_size as usize]))
    }
}

/// NtQuerySystemInformation(SystemProcessIdInformation) 输入结构（repr(C) 与 C 布局一致）。
#[repr(C)]
struct NtUnicodeString {
    length: u16,
    maximum_length: u16,
    buffer: *mut u16,
}

#[repr(C)]
struct SystemProcessIdInformation {
    process_id: *mut core::ffi::c_void,
    image_name: NtUnicodeString,
}

/// 兜底：NtQuerySystemInformation(SystemProcessIdInformation) 查询 exe 路径。
/// 纯内核查询，无需打开目标进程句柄，可覆盖 PPL 等 OpenProcess 被拒的受保护进程
/// （返回 NT 设备路径，由 normalize_image_path 转回盘符）。
fn query_exe_path_by_nt(pid: u32) -> Option<String> {
    use ntapi::ntexapi::{NtQuerySystemInformation, SystemProcessIdInformation};

    unsafe {
        let mut name_buf = [0u16; 2048];
        let mut info = SystemProcessIdInformation {
            process_id: pid as usize as *mut core::ffi::c_void,
            image_name: NtUnicodeString {
                length: 0,
                maximum_length: (name_buf.len() * 2) as u16,
                buffer: name_buf.as_mut_ptr(),
            },
        };
        let mut ret: u32 = 0;
        let status = NtQuerySystemInformation(
            SystemProcessIdInformation,
            &mut info as *mut _ as *mut ntapi::winapi::ctypes::c_void,
            std::mem::size_of::<SystemProcessIdInformation>() as u32,
            &mut ret,
        );
        if status != 0 {
            crate::process::append_verbose_log(&format!(
                "[app_icon] NtQuerySystemInformation pid={pid} 失败 status={:#x}",
                status as u32
            ));
            return None;
        }
        let len = info.image_name.length as usize / 2;
        if len == 0 {
            return None;
        }
        normalize_image_path(&String::from_utf16_lossy(&name_buf[..len]))
    }
}

/// NT 设备路径（\Device\HarddiskVolumeN\...）转盘符路径；已有盘符则原样返回。
fn normalize_image_path(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetLogicalDriveStringsW, QueryDosDeviceW};

    // \??\ 与 \\?\ 均为 Win32 命名空间前缀，剥掉即为盘符路径
    if let Some(rest) = path
        .strip_prefix("\\\\?\\")
        .or_else(|| path.strip_prefix("\\??\\"))
    {
        return Some(rest.to_string());
    }

    // 已含盘符（"C:\..."）
    if path.as_bytes().get(1) == Some(&b':') {
        return Some(path.to_string());
    }

    // \Device\HarddiskVolumeN\... → 盘符
    let rest = match path.strip_prefix("\\Device\\") {
        Some(r) => r,
        None => {
            crate::process::append_verbose_log(&format!("[app_icon] 未知路径形态: {path}"));
            return Some(path.to_string());
        }
    };
    let mut parts = rest.splitn(2, '\\');
    let volume = parts.next().unwrap_or("");
    let after = parts.next().unwrap_or("");
    if volume.is_empty() {
        return Some(path.to_string());
    }
    let device = format!("\\Device\\{volume}");

    unsafe {
        let mut drives = [0u16; 512];
        let n = GetLogicalDriveStringsW(Some(&mut drives));
        if n == 0 {
            crate::process::append_verbose_log(&format!(
                "[app_icon] GetLogicalDriveStringsW 失败: {path}"
            ));
            return None;
        }
        let mut cur = 0usize;
        while cur < n as usize {
            let s = &drives[cur..];
            let len = s.iter().position(|&c| c == 0).unwrap_or(s.len());
            if len == 0 {
                break;
            }
            let drive = String::from_utf16_lossy(&s[..len]); // "C:\"
            let drive_root = drive.trim_end_matches('\\');
            let wide: Vec<u16> = crate::process::to_wide(drive_root);
            let mut target = [0u16; 512];
            let t = QueryDosDeviceW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut target));
            if t > 0 {
                // 返回长度 t 跨 Windows 版本可能含结尾 NUL，故以首个 \0 为准截断
                let end = target.iter().position(|&c| c == 0).unwrap_or(target.len());
                let target_str = String::from_utf16_lossy(&target[..end]);
                if target_str.eq_ignore_ascii_case(&device) {
                    return Some(format!("{drive}{after}"));
                }
            }
            cur += len + 1;
        }
    }

    crate::process::append_verbose_log(&format!("[app_icon] 设备路径转盘符失败: {path}"));
    None
}

/// 从进程 PID 查询 exe 路径：主路径（OpenProcess）→ 内核兜底（NtQuerySystemInformation）。
fn query_exe_path_by_pid(pid: u32) -> Option<String> {
    query_exe_path_by_openprocess(pid).or_else(|| {
        crate::process::append_verbose_log(&format!(
            "[app_icon] 常规查询失败 pid={pid}，走 NtQuerySystemInformation 兜底"
        ));
        query_exe_path_by_nt(pid)
    })
}

/// 从进程PID解析应用名称
fn resolve_process_name(pid: u32) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{GetFileVersionInfoSizeW, GetFileVersionInfoW};

    let exe_path = query_exe_path_by_pid(pid)?;

    // 读取文件版本信息中的 FileDescription（如 "Google Chrome"）
    unsafe {
        let wide_path: Vec<u16> = crate::process::to_wide(&exe_path);
        let size = GetFileVersionInfoSizeW(windows::core::PCWSTR(wide_path.as_ptr()), None);
        if size > 0 {
            let mut data = vec![0u8; size as usize];
            if GetFileVersionInfoW(
                windows::core::PCWSTR(wide_path.as_ptr()),
                None,
                size,
                data.as_mut_ptr() as *mut _,
            )
            .is_ok()
            {
                if let Some(name) = query_file_description(&data) {
                    return Some(name);
                }
            }
        }
    }

    // 回退：exe 文件名（去掉扩展名）
    std::path::Path::new(&exe_path)
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
}

/// 从版本信息数据中查询 FileDescription
unsafe fn query_file_description(data: &[u8]) -> Option<String> {
    use windows::Win32::Storage::FileSystem::VerQueryValueW;

    let mut buf: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len: u32 = 0;
    let ok = VerQueryValueW(
        data.as_ptr() as *const _,
        windows::core::w!("\\VarFileInfo\\Translation"),
        &mut buf,
        &mut len,
    );
    if !ok.as_bool() || buf.is_null() || len < 4 {
        return None;
    }
    let lang = (buf as *const u16).read();
    let codepage = (buf as *const u16).add(1).read();
    let key = format!(
        "\\StringFileInfo\\{:04X}{:04X}\\FileDescription",
        lang, codepage
    );
    let key_wide: Vec<u16> = crate::process::to_wide(&key);
    let mut buf2: *mut core::ffi::c_void = std::ptr::null_mut();
    let mut len2: u32 = 0;
    let ok2 = VerQueryValueW(
        data.as_ptr() as *const _,
        windows::core::PCWSTR(key_wide.as_ptr()),
        &mut buf2,
        &mut len2,
    );
    if !ok2.as_bool() || buf2.is_null() {
        return None;
    }
    let name = String::from_utf16_lossy(std::slice::from_raw_parts(
        buf2 as *const u16,
        len2 as usize,
    ));
    let name = name.split('\0').next().unwrap_or("").trim().to_string();
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

/// 从进程PID获取应用图标（返回base64编码的PNG）
pub fn get_app_icon_by_pid(pid: u32) -> Option<Arc<str>> {
    let cache =
        ICON_CACHE.get_or_init(|| Mutex::new(LruCache::new(NonZeroUsize::new(256).unwrap())));
    {
        let mut guard = crate::state::lock_unpoisoned(cache);
        if let Some(icon) = guard.get(&pid) {
            return Some(Arc::clone(icon));
        }
    }
    let icon: Option<Arc<str>> = (|| -> Option<Arc<str>> {
        let exe_path = query_exe_path_by_pid(pid)?;
        let icon = get_icon_from_path(&exe_path);
        if icon.is_none() {
            crate::process::append_verbose_log(&format!(
                "[app_icon] 取图失败 pid={pid} path={exe_path}"
            ));
        }
        icon
    })();
    icon.as_ref()?;
    let mut guard = crate::state::lock_unpoisoned(cache);
    guard.put(pid, Arc::clone(icon.as_ref().unwrap()));
    icon
}

/// 从文件路径提取图标（返回base64编码的PNG）
fn get_icon_from_path(path: &str) -> Option<Arc<str>> {
    unsafe {
        // 注：windows crate 将 PrivateExtractIconsW 的 szfilename 固定为 &[u16; 260]，
        // 故取图阶段的路径缓冲无法放大；长路径(>259)的 exe 会在此截断失败（罕见）。
        // 路径查询阶段（query_exe_path_by_pid）已支持长路径。
        let mut path_buf = [0u16; 260];
        let path_wide: Vec<u16> = crate::process::to_wide(path);
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
unsafe fn get_icon_bitmap(
    hicon: windows::Win32::UI::WindowsAndMessaging::HICON,
) -> Option<RgbaImage> {
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
        ReleaseDC(
            Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())),
            hdc_screen,
        );
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
    )
    .ok()?;
    if hbitmap.is_invalid() {
        let _ = DeleteDC(hdc_mem);
        let _ = ReleaseDC(
            Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())),
            hdc_screen,
        );
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
    let _ = ReleaseDC(
        Some(windows::Win32::Foundation::HWND(std::ptr::null_mut())),
        hdc_screen,
    );

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
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .ok()?;

    use base64::Engine;
    Some(
        base64::engine::general_purpose::STANDARD
            .encode(buffer.into_inner())
            .into(),
    )
}

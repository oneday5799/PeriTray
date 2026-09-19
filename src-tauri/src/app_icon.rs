use crate::verbose_log;
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
                verbose_log!("[app_icon] OpenProcess pid={pid} 失败（权限/保护进程）");
                return None;
            }
        };
        // ── 为什么用堆上的 `Vec<u16>` 而不是 `[0u16; 32768]`（P3-5）──────
        // 32768 个 `u16` = **64 KiB**，直接压在栈上。这个函数是**逐进程**调用的
        // （会话列表里通常几十个 PID），而且调用点分布在 Tauri 命令 / 托盘刷新等
        // 线程上 —— 栈默认 1 MiB（后台线程常被调小到 256 KiB），64 KiB 的
        // 单帧占用完全是浪费，也埋着栈溢出的风险。
        //
        // `vec![0u16; n]` 是一次性堆分配，容量由 Windows 规定的路径上限
        // （`MAX_PATH` 扩展名 32767 宽字符）决定，既不给栈压力，也不牺牲长路径。
        let mut path_buf = vec![0u16; 32768];
        let mut path_size = path_buf.len() as u32;
        let result = QueryFullProcessImageNameW(
            process_handle,
            PROCESS_NAME_FORMAT(0),
            windows::core::PWSTR(path_buf.as_mut_ptr()),
            &mut path_size,
        );
        let _ = windows::Win32::Foundation::CloseHandle(process_handle);
        if result.is_err() {
            verbose_log!("[app_icon] QueryFullProcessImageNameW pid={pid} 失败");
            return None;
        }
        // `path_size` 是**不含 NUL 的字符数**，且 API 成功时必然 <= 缓冲长度 ⇒
        // 这里的切片不会越界。截断是 API 的契约（超出上限会失败而不是静默截断），
        // 故无需额外的长度校验。
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

/// 校验内核返回的字符长度是否可安全用于切片。
///
/// 抽成**真函数**而不是在 `query_exe_path_by_nt` 里内联一个 `if`：
/// 内联时单测只能「照抄一遍判据」来断言，那种测试对判据的改动是**免疫的**
/// （把 `>` 改成 `>=` 也照样绿）—— 是假验收。抽出来后单测调用的是真判据。
///
/// 边界取**严格大于**：`len == cap` 时 `&buf[..len]` 恰好取满，合法。
fn nt_length_in_bounds(len: usize, cap: usize) -> bool {
    len <= cap
}

/// 兜底：NtQuerySystemInformation(SystemProcessIdInformation) 查询 exe 路径。
/// 纯内核查询，无需打开目标进程句柄，可覆盖 PPL 等 OpenProcess 被拒的受保护进程
/// （返回 NT 设备路径，由 normalize_image_path 转回盘符）。
fn query_exe_path_by_nt(pid: u32) -> Option<String> {
    use ntapi::ntexapi::{NtQuerySystemInformation, SystemProcessIdInformation};

    unsafe {
        // 同理改堆分配（P3-5）：4 KiB 压栈虽不致命，但这条路径本身就在
        // 「OpenProcess 被拒」的兜底分支上，不该再给栈加无谓开销。
        let mut name_buf = vec![0u16; 2048];
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
            verbose_log!(
                "[app_icon] NtQuerySystemInformation pid={pid} 失败 status={:#x}",
                status as u32
            );
            return None;
        }
        let len = info.image_name.length as usize / 2;
        if len == 0 {
            return None;
        }
        // 内核返回的 `length` 理论上不会超过我们给的 `maximum_length`，但这是
        // **内核数据结构**：若真被填成更大的值，下面的切片就会越界 panic
        // （而 `unsafe` 块内的越界读是 UB）。宁可保守地判成「查不到」，
        // 也不要拿一个无法证伪的前提去切片。
        if !nt_length_in_bounds(len, name_buf.len()) {
            verbose_log!(
                "[app_icon] NtQuerySystemInformation pid={pid} 返回长度异常 {} > 缓冲 {}",
                len,
                name_buf.len()
            );
            return None;
        }
        normalize_image_path(&String::from_utf16_lossy(&name_buf[..len]))
    }
}

/// 把 `\Device\HarddiskVolumeN\rest` 拆成 `(volume, after)`，供盘符映射使用。
///
/// 抽成不依赖任何 Win32 调用的纯函数（P3-5 附带）：`normalize_image_path` 的
/// 前两步（剥命名空间前缀、识别已含盘符）逻辑简单，但第三步的拆分一旦
/// 写错（比如 `\Device\` 后面没有反斜杠、或 `volume` 为空）就会退化成
/// 「返回原路径」或「返回 None」，只能靠在 Windows 上真机跑才能发现。
/// 拆出来后这些边界可以在任何平台单测。
fn split_device_path(path: &str) -> Option<(&str, &str)> {
    let rest = path.strip_prefix("\\Device\\")?;
    let mut parts = rest.splitn(2, '\\');
    let volume = parts.next().unwrap_or("");
    if volume.is_empty() {
        return None;
    }
    let after = parts.next().unwrap_or("");
    Some((volume, after))
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
    let Some((volume, after)) = split_device_path(path) else {
        verbose_log!("[app_icon] 未知路径形态: {path}");
        return Some(path.to_string());
    };
    let device = format!("\\Device\\{volume}");

    unsafe {
        let mut drives = [0u16; 512];
        let n = GetLogicalDriveStringsW(Some(&mut drives));
        if n == 0 {
            verbose_log!("[app_icon] GetLogicalDriveStringsW 失败: {path}");
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

    verbose_log!("[app_icon] 设备路径转盘符失败: {path}");
    None
}

/// 从进程 PID 查询 exe 路径：主路径（OpenProcess）→ 内核兜底（NtQuerySystemInformation）。
fn query_exe_path_by_pid(pid: u32) -> Option<String> {
    query_exe_path_by_openprocess(pid).or_else(|| {
        verbose_log!("[app_icon] 常规查询失败 pid={pid}，走 NtQuerySystemInformation 兜底");
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
            verbose_log!("[app_icon] 取图失败 pid={pid} path={exe_path}");
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
    // 用 as_chunks_mut 而非 chunks_exact_mut：块长 4 在编译期已知，且 clippy 1.98 新增的
    // chunks_exact_to_as_chunks 正是建议此写法。语义等价——两者都跳过末尾不足 4 字节的余数。
    for chunk in pixels.as_chunks_mut::<4>().0 {
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

#[cfg(test)]
mod tests {
    use super::*;

    // ── P3-5：`\Device\...` 路径拆分的边界 ──────────────────────────

    #[test]
    fn split_device_path_splits_volume_and_rest() {
        assert_eq!(
            split_device_path("\\Device\\HarddiskVolume3\\Windows\\explorer.exe"),
            Some(("HarddiskVolume3", "Windows\\explorer.exe"))
        );
        // 卷根（after 为空）是合法形态，不该被判成 None
        assert_eq!(
            split_device_path("\\Device\\HarddiskVolume3"),
            Some(("HarddiskVolume3", ""))
        );
        assert_eq!(
            split_device_path("\\Device\\HarddiskVolume3\\"),
            Some(("HarddiskVolume3", ""))
        );
    }

    #[test]
    fn split_device_path_rejects_non_device_and_empty_volume() {
        // 不是 \Device\ 前缀
        assert_eq!(split_device_path("C://Windows//explorer.exe"), None);
        assert_eq!(split_device_path("\\??\\C://x.exe"), None);
        assert_eq!(split_device_path(""), None);
        // 前缀后为空卷名
        assert_eq!(split_device_path("\\Device\\"), None);
        assert_eq!(split_device_path("\\Device\\\\x.exe"), None);
    }

    /// 拆分结果拼回去必须与输入一致（`after` 为空时不能多出反斜杠）。
    #[test]
    fn split_device_path_round_trips() {
        for p in [
            "\\Device\\HarddiskVolume1\\a\\b\\c.exe",
            "\\Device\\HarddiskVolume10\\",
            "\\Device\\HarddiskVolume2",
        ] {
            let (vol, after) = split_device_path(p).expect("应能拆分");
            let rebuilt = if after.is_empty() {
                if p.ends_with('\\') {
                    format!("\\Device\\{vol}\\")
                } else {
                    format!("\\Device\\{vol}")
                }
            } else {
                format!("\\Device\\{vol}\\{after}")
            };
            assert_eq!(rebuilt, p, "拆分后拼回应还原原路径");
        }
    }

    /// 长路径（远超 MAX_PATH）必须能完整往返 —— 堆缓冲的意义就在这里。
    ///
    /// 注：`query_exe_path_by_openprocess` 本身要真实 PID 才能跑，
    /// 但「缓冲大小是否够、切片是否完整」这件事与 Win32 无关，
    /// 用同构的 `Vec<u16>` + `from_utf16_lossy` 即可覆盖。
    #[test]
    fn long_path_survives_wide_round_trip() {
        let long = format!("C://{}", "a\\".repeat(4000));
        assert!(long.len() > 8000, "构造的路径应远超 MAX_PATH");
        let wide: Vec<u16> = long.encode_utf16().collect();
        // 模拟 API：写入 wide 后 length = 字符数（不含 NUL）
        let buf = vec![0u16; 32768];
        let mut buf = buf;
        buf[..wide.len()].copy_from_slice(&wide);
        let path_size = wide.len() as u32;
        let decoded = String::from_utf16_lossy(&buf[..path_size as usize]);
        assert_eq!(decoded, long, "超长路径应完整还原，不 panic 不截断");
        assert_eq!(decoded.len(), long.len());
    }

    /// `query_exe_path_by_nt` 的长度守卫：`length` 若被填得比缓冲还大，
    /// 必须判成「查不到」而不是越界切片。这里直接验算守卫判据本身。
    #[test]
    fn nt_length_guard_rejects_oversized_reports() {
        // 调用的是**真判据** `nt_length_in_bounds`（不是照抄一遍条件）：
        // 照抄式的测试对判据改动免疫（把 `<=` 改成 `<` 也照样绿），等于没测。
        let cap = 2048usize;
        // 两侧都断言：容量内合法、恰好等于容量合法、超 1 即非法
        assert!(nt_length_in_bounds(0, cap), "空长度合法");
        assert!(nt_length_in_bounds(1, cap), "正常短名合法");
        assert!(nt_length_in_bounds(cap - 1, cap), "差 1 合法");
        assert!(
            nt_length_in_bounds(cap, cap),
            "恰好等于容量合法（&buf[..cap] 恰好取满）"
        );
        assert!(
            !nt_length_in_bounds(cap + 1, cap),
            "超出 1 即非法，否则切片越界"
        );
        assert!(!nt_length_in_bounds(usize::MAX, cap), "极端值不得判为合法");
    }
}

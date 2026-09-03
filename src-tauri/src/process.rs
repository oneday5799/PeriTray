//! 进程级基础工具集（模块名沿用历史）：日志子系统（append_log/clean_old_logs）、
//! 本地时间戳 chrono_str（GetLocalTime 直取系统本地时间）、exe 路径、
//! Win32 互操作（to_wide/shell_open）与各类系统面板/文件打开器。
//! 为全仓约三分之二模块提供公共依赖，新增跨模块基础工具优先落于此处。

use std::os::windows::ffi::OsStrExt;
use std::path::PathBuf;
use std::process::Command;

/// 获取 exe 所在目录
pub fn exe_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|p| p.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// 获取日志目录（`<exe目录>/logs`）
pub fn logs_dir() -> PathBuf {
    exe_dir().join("logs")
}

/// 获取数据目录（`<exe目录>/data`）
pub fn data_dir() -> PathBuf {
    exe_dir().join("data")
}

/// 创建 Windows 隐藏窗口命令
#[cfg(target_os = "windows")]
fn new_hidden_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

/// 获取日志文件路径（写入 logs/ 子目录；once 为 debug_once_{pid}.log，其余按天 debug_YYYYMMDD.log）
fn log_path() -> std::path::PathBuf {
    let dir = logs_dir();
    if crate::config::log_once() {
        dir.join(format!("debug_once_{}.log", std::process::id()))
    } else {
        dir.join(format!("debug_{}.log", local_date_str()))
    }
}

/// 追加日志到文件（标准级：生命周期摘要与各模块常规行）
pub fn append_log(msg: &str) {
    if !crate::config::standard_log_enabled() {
        return;
    }
    write_log(msg);
}

/// 追加诊断日志到文件（详细级：逐路径/轮次/缓存决策等现场细节）
pub fn append_verbose_log(msg: &str) {
    if !crate::config::verbose_log_enabled() {
        return;
    }
    write_log(msg);
}

/// 落盘失败计数：首次失败告警，后续静默（防止高频日志重复刷屏）
static LOG_WRITE_FAILS: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

fn write_log(msg: &str) {
    use std::io::Write;
    let timestamp = chrono_str();
    let line = format!("[{}]{}\n", timestamp, msg);
    let path = log_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut ok = false;
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        ok = file.write_all(line.as_bytes()).is_ok();
    }
    // 首次落盘失败时 stderr 直出告警，防止日志丢失无感知
    if !ok && !LOG_WRITE_FAILS.swap(true, std::sync::atomic::Ordering::SeqCst) {
        eprintln!("[process] 日志写入失败，日志可能丢失: {:?}", path);
    }
}

/// 清理旧日志文件（根据保留时长设置）
pub fn clean_old_logs() {
    use crate::config::LogRetention;

    let retention = crate::config::with_config(|c| c.log_retention);

    // 根目录遗留：旧版本把日志写在 exe 根目录，这里无条件清除，避免根目录杂乱
    remove_legacy_root_logs();

    let entries = match std::fs::read_dir(logs_dir()) {
        Ok(e) => e,
        Err(_) => return,
    };

    let current_name = log_path().file_name().map(|n| n.to_owned());
    let (y, m, d) = local_date();
    let today_days = days_from_civil(y as i64, m as i64, d as i64);

    for entry in entries.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // 跳过当前正在写入的日志文件（任何触发时机都不误删活动日志）
        if current_name.as_deref() == Some(name.as_os_str()) {
            continue;
        }
        if !(name_str.starts_with("debug") && name_str.ends_with(".log")) {
            continue;
        }

        let delete = match retention {
            LogRetention::Once => true,
            _ => match parse_log_date(&name_str) {
                Some((fy, fm, fd)) => {
                    today_days - days_from_civil(fy as i64, fm as i64, fd as i64)
                        >= retention_days(retention)
                }
                // 非日期命名（debug_once_*、debug.log、历史 debug_{pid}.log）一律视为旧格式删除
                None => true,
            },
        };

        if delete {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// 清除 exe 根目录下旧版本遗留的 debug*.log（迁移至 logs/ 前的历史文件）
fn remove_legacy_root_logs() {
    if let Ok(entries) = std::fs::read_dir(exe_dir()) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("debug")
                && name_str.ends_with(".log")
                && entry.file_type().map(|t| t.is_file()).unwrap_or(false)
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// 保留时长 → 天数（Once 不在此路径，其值仅占位）
fn retention_days(r: crate::config::LogRetention) -> i64 {
    match r {
        crate::config::LogRetention::OneDay => 1,
        crate::config::LogRetention::ThreeDays => 3,
        crate::config::LogRetention::OneWeek => 7,
        crate::config::LogRetention::OneMonth => 30,
        crate::config::LogRetention::Once => 0,
    }
}

/// 从 `debug_YYYYMMDD.log` 解析日期；不是「debug_ + 8 位数字 + .log」则返回 None。
/// once 文件为 `debug_once_{pid}.log`（前缀天然区分），无需日历合法性校验。
fn parse_log_date(name: &str) -> Option<(i32, i32, i32)> {
    let rest = name.strip_prefix("debug_")?.strip_suffix(".log")?;
    if rest.len() != 8 || !rest.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let y = rest[0..4].parse::<i32>().ok()?;
    let m = rest[4..6].parse::<i32>().ok()?;
    let d = rest[6..8].parse::<i32>().ok()?;
    Some((y, m, d))
}

/// 本地日期（年、月、日）。Windows 直取 GetLocalTime；非 Windows 由 epoch 天数反解。
fn local_date() -> (i32, i32, i32) {
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        (st.wYear as i32, st.wMonth as i32, st.wDay as i32)
    }
    #[cfg(not(target_os = "windows"))]
    {
        let Ok(dur) =
            std::time::SystemTime::now().duration_since(std::time::SystemTime::UNIX_EPOCH)
        else {
            return (1970, 1, 1);
        };
        let days: i64 = (dur.as_secs() / 86400) as i64 + 719_468;
        let era = days.div_euclid(146_097);
        let doe = days - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
        let mut y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let m = if mp < 10 { mp + 3 } else { mp - 9 };
        if m <= 2 {
            y += 1;
        }
        (y as i32, m as i32, d as i32)
    }
}

/// 本地日期字符串 YYYYMMDD（用于按天轮转的日志文件名）
fn local_date_str() -> String {
    let (y, m, d) = local_date();
    format!("{:04}{:02}{:02}", y, m, d)
}

/// civil 日期 → 自 1970-01-01 的天数（Hinnant 算法；仅用于求差，偏移常量不影响正确性）
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = if m > 2 { m - 3 } else { m + 9 };
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn chrono_str() -> String {
    // 直接使用系统本地时间，避免手动 UTC 偏移计算的边界问题
    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::Foundation::SYSTEMTIME;
        use windows_sys::Win32::System::SystemInformation::GetLocalTime;
        let mut st: SYSTEMTIME = unsafe { std::mem::zeroed() };
        unsafe { GetLocalTime(&mut st) };
        return format!(
            "{:04}.{:02}.{:02} {:02}:{:02}:{:02}.{:03}",
            st.wYear, st.wMonth, st.wDay, st.wHour, st.wMinute, st.wSecond, st.wMilliseconds
        );
    }
    #[cfg(not(target_os = "windows"))]
    {
        use std::time::SystemTime;
        let Ok(dur) = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH) else {
            return "????.??.?? ??:??:??".into();
        };
        let secs = dur.as_secs();
        let h = (secs / 3600) % 24;
        let min = (secs / 60) % 60;
        let s = secs % 60;
        let days = secs / 86400 + 719468;
        let era = days / 146097;
        let doe = days - era * 146097;
        let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
        let y = yoe + era * 400;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let d = doy - (153 * mp + 2) / 5 + 1;
        let mon = if mp < 10 { mp + 3 } else { mp - 9 };
        let y = if mon <= 2 { y + 1 } else { y };
        format!("{:04}.{:02}.{:02} {:02}:{:02}:{:02}", y, mon, d, h, min, s)
    }
}

/// 使用系统默认程序打开文件/URL
pub fn open_with_system(path: &str) -> Result<(), String> {
    let mut cmd = new_hidden_cmd("cmd");
    cmd.args(["/c", "start", "", path]).spawn().map_err(|e| {
        crate::process::append_log(&format!(
            "[process] open_with_system failed: {} -> {}",
            path, e
        ));
        e.to_string()
    })?;
    Ok(())
}

/// 将字符串转换为 Windows 宽字符串 (null-terminated UTF-16)
pub fn to_wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect()
}

/// 动态加载 DLL 并返回模块句柄（封装 kernel32 LoadLibraryA；name 须以 `\0` 结尾）
pub unsafe fn load_library(name: &[u8]) -> *mut core::ffi::c_void {
    LoadLibraryA(name.as_ptr())
}

/// 按名取 DLL 导出函数地址（封装 kernel32 GetProcAddress；name 须以 `\0` 结尾）
pub unsafe fn get_proc_address(
    module: *mut core::ffi::c_void,
    name: &[u8],
) -> *mut core::ffi::c_void {
    GetProcAddress(module, name.as_ptr())
}

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut core::ffi::c_void;
    fn GetProcAddress(module: *mut core::ffi::c_void, name: *const u8) -> *mut core::ffi::c_void;
}

/// 通过 ShellExecuteW 打开文件/URL/命令
fn shell_open(file: &str, params: Option<&str>) {
    let wide_file = to_wide(file);
    let wide_params = params.map(to_wide);
    let wide_verb = to_wide("open");
    unsafe {
        windows_sys::Win32::UI::Shell::ShellExecuteW(
            std::ptr::null_mut(),
            wide_verb.as_ptr(),
            wide_file.as_ptr(),
            wide_params
                .as_ref()
                .map_or(std::ptr::null(), |v| v.as_ptr()),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

/// 打开旧版声音控制面板 (mmsys.cpl)
pub fn open_sound_panel(panel: &str) {
    shell_open(
        "rundll32.exe",
        Some(&format!("shell32.dll,Control_RunDLL mmsys.cpl,,{}", panel)),
    );
}

/// 打开现代 Windows 设置页面 (ms-settings:)
pub fn open_settings_page(page: &str) {
    shell_open(&format!("ms-settings:{}", page), None);
}

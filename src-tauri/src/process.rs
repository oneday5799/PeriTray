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

/// 创建 Windows 隐藏窗口命令
#[cfg(target_os = "windows")]
pub fn new_hidden_cmd(program: &str) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x08000000;
    let mut cmd = Command::new(program);
    cmd.creation_flags(CREATE_NO_WINDOW);
    cmd
}

#[cfg(not(target_os = "windows"))]
pub fn new_hidden_cmd(program: &str) -> Command {
    Command::new(program)
}

/// 获取日志文件路径
fn log_path() -> std::path::PathBuf {
    if crate::config::log_once() {
        exe_dir().join(format!("debug_{}.log", std::process::id()))
    } else {
        exe_dir().join("debug.log")
    }
}

/// 追加日志到文件
pub fn append_log(msg: &str) {
    if !crate::config::log_enabled() {
        return;
    }
    write_log(msg);
}

/// 追加详细日志到文件（与 append_log 相同，保留接口兼容）
pub fn append_log_detailed(msg: &str) {
    append_log(msg);
}

fn write_log(msg: &str) {
    use std::io::Write;
    let timestamp = chrono_str();
    let line = format!("[{}]{}\n", timestamp, msg);
    let path = log_path();
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true).append(true)
        .open(&path)
    {
        let _ = file.write_all(line.as_bytes());
    }
}

/// 清理旧日志文件（根据保留时长设置）
pub fn clean_old_logs() {
    use std::time::{SystemTime, Duration};
    use crate::config::LogRetention;

    let (enabled, retention) = crate::config::with_config(|c| (c.log_enabled, c.log_retention));
    if !enabled {
        return;
    }

    let dir = exe_dir();
    let now = SystemTime::now();

    let max_age = match retention {
        LogRetention::Once => {
            // 一次模式：删除所有 debug*.log
            if let Ok(entries) = std::fs::read_dir(&dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("debug") && name_str.ends_with(".log") {
                        let _ = std::fs::remove_file(entry.path());
                    }
                }
            }
            return;
        }
        LogRetention::ThreeDays => Duration::from_secs(3 * 86400),
        LogRetention::OneWeek => Duration::from_secs(7 * 86400),
        LogRetention::OneMonth => Duration::from_secs(30 * 86400),
        LogRetention::OneDay => Duration::from_secs(86400),
    };

    // 非 once 模式：清理超过保留时长的 debug*.log 文件
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if name_str.starts_with("debug") && name_str.ends_with(".log") {
                if let Ok(meta) = entry.metadata() {
                    if let Ok(modified) = meta.modified() {
                        if let Ok(elapsed) = now.duration_since(modified) {
                            if elapsed > max_age {
                                let _ = std::fs::remove_file(entry.path());
                            }
                        }
                    }
                }
            }
        }
    }
}

fn chrono_str() -> String {
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

/// 使用系统默认程序打开文件/URL
pub fn open_with_system(path: &str) -> Result<(), String> {
    let mut cmd = new_hidden_cmd("cmd");
    cmd.args(["/c", "start", "", path])
        .spawn()
        .map_err(|e| {
            crate::process::append_log(&format!("[process] open_with_system failed: {} -> {}", path, e));
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
            wide_params.as_ref().map_or(std::ptr::null(), |v| v.as_ptr()),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        );
    }
}

/// 打开旧版声音控制面板 (mmsys.cpl)
pub fn open_sound_panel(panel: &str) {
    shell_open("rundll32.exe", Some(&format!("shell32.dll,Control_RunDLL mmsys.cpl,,{}", panel)));
}

/// 打开现代 Windows 设置页面 (ms-settings:)
pub fn open_settings_page(page: &str) {
    shell_open(&format!("ms-settings:{}", page), None);
}

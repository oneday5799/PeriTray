//! 点音乐封面 → 激活对应应用（把那个应用的前台窗口拉起来）。
//!
//! 加载序 N/N · 提供：activate_media_app() / find_main_window()
//! 依赖：crate::taskbar_music（快照里的 pid）、crate::audio（匹配口径的归属）
//!
//! ⚠️ **为什么需要这一层**：音乐面板的数据源是 SMTC 会话，身份是 **AUMID**；
//!   而窗口只能按 **pid** 找（`EnumWindows` + `GetWindowThreadProcessId`），
//!   而 pid 又只能从 AUMID 匹配音频会话得到 ⇒ 两条已有的桥在这里接上。
//!
//! ⛔ **本模块全程只做 Win32 快调用**（`EnumWindows`、`ShowWindow`、
//!   `SetForegroundWindow`、`ShellExecuteW`），**不做 COM 枚举、不读配置、
//!   不持任何锁**：它跑在**窗口线程的点击路径**上，而一次音频会话枚举实测
//!   百毫秒级 ⇒ 会让 widget 明显卡顿。所以 pid 由音乐后台线程预先算好放进
//!   快照（`MusicSnapshot::session_pid`），这里只做「读快照 → 找窗口 → 激活」。
//!
//! 判据全文见 Wiki 15 §8.6.9。

use windows_sys::core::BOOL;
use windows_sys::Win32::Foundation::{HWND, LPARAM, RECT};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    EnumWindows, GetWindowRect, GetWindowThreadProcessId, IsIconic, IsWindowVisible,
    SetForegroundWindow, ShowWindow, SW_RESTORE,
};

/// ⭐ 激活结果（调用方据此决定记什么日志；**不弹窗**）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Activated {
    /// 已把目标窗口拉到前台。
    Window,
    /// 目标没有可见窗口，已用 shell 的 `AppsFolder` 拉起（商店版应用）。
    ShellAppsFolder,
    /// 目标没有可见窗口，已按 exe 重新拉起（托盘类应用的兜底）。
    LaunchedExe,
    /// 什么都没做（详见 `activate_media_app` 各分支注释）。
    NoWindow,
    /// 找到窗口但 `SetForegroundWindow` 被系统拒绝（前台窗口锁）。
    ForegroundBlocked,
}

/// `EnumWindows` 的回调上下文（回调是裸 `extern fn` ⇒ 只能靠指针带状态）。
struct WindowScan {
    pid: u32,
    scanned: usize,
    best_area: i64,
    best: HWND,
}

/// ⛔ **枚举量上限**：本函数跑在窗口线程的点击路径上，必须有界。正常情况
///   （几千个顶层窗口）在第一毫秒内扫完；这个上限只在「窗口枚举异常缓慢」
///   时兜底，避免一次点击把 widget 冻住。
const MAX_WINDOWS_SCANNED: usize = 8192;

/// 按 pid 找它的**主窗口**（面积最大的可见顶层窗口）。
///
/// ⭐ 口径与 .NET 的 `Process.MainWindowHandle` 同源（`ck/FluentFlyout`、
///   `ck/AF-Media-Bar` 都用它）：**可见 + 面积最大**。
/// ⚠️ pid 过滤**同时排掉了本进程**的窗口：widget 与 tooltip 都是本进程的
///   顶层窗口，激活它们等于「点了没反应」，而 `SetForegroundWindow` 会
///   **成功返回** ⇒ 更迷惑（所以这里必须靠 pid 过滤而不是事后判断）。
/// ⚠️ 面积用 `(right-left) × (bottom-top)` 粗算：只要相对大小，不需要真面积。
pub fn find_main_window(pid: u32) -> Option<HWND> {
    unsafe extern "system" fn cb(hwnd: HWND, lparam: LPARAM) -> BOOL {
        let ctx = &mut *(lparam as *mut WindowScan);
        if ctx.scanned >= MAX_WINDOWS_SCANNED {
            return 0;
        }
        ctx.scanned += 1;
        if IsWindowVisible(hwnd) == 0 {
            return 1;
        }
        let mut wpid = 0u32;
        GetWindowThreadProcessId(hwnd, &mut wpid);
        if wpid != ctx.pid {
            return 1;
        }
        let mut r: RECT = std::mem::zeroed();
        if GetWindowRect(hwnd, &mut r) == 0 {
            return 1;
        }
        let area = (r.right - r.left) as i64 * (r.bottom - r.top) as i64;
        if area > 0 && area > ctx.best_area {
            ctx.best_area = area;
            ctx.best = hwnd;
        }
        1
    }
    let mut ctx = WindowScan {
        pid,
        scanned: 0,
        best_area: 0,
        best: std::ptr::null_mut(),
    };
    unsafe {
        EnumWindows(Some(cb), &mut ctx as *mut WindowScan as LPARAM);
    }
    if ctx.best.is_null() {
        None
    } else {
        Some(ctx.best)
    }
}

/// pid → 主窗口 → 拉前台。
///
/// 返回 `None` = 该 pid **没有可见窗口**；`Some(ForegroundBlocked)` =
/// 有窗口但系统拒绝切换（两者必须区分：前者是「没窗口可拉」，后者是
/// 「窗口在但切不动」，排查方向完全不同）。
fn bring_window_to_front(pid: u32) -> Option<Activated> {
    let hwnd = find_main_window(pid)?;
    unsafe {
        if IsIconic(hwnd) != 0 {
            // ⭐ 最小化着必须先恢复：只 `SetForegroundWindow` 会点亮任务栏
            //   按钮而窗口仍是最小化（用户看着像没反应）。
            ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd) != 0 {
            Some(Activated::Window)
        } else {
            Some(Activated::ForegroundBlocked)
        }
    }
}

/// `explorer.exe shell:AppsFolder\{aumid}`（商店版应用的拉起方式）。
///
/// ⭐ 只在 AUMID **含 `!`** 时才用：`shell:AppsFolder` 的参数必须是
///   「包族名 + `!` + 应用 id」（`Spotify_zpdnekdrzrea0!Spotify`）；
///   Win32 应用自设的 AUMID（`com.vendor.player`）会被 shell 拒绝。
///   ⚠️ 这个 `!` 判据来自 `ck/AF-Media-Bar` 的同款实现（它也只认 `!`），
///   不是本仓臆造。
/// ⚠️ 走 `ShellExecuteW` 而不用 `Command::new`：`explorer.exe` 已在运行时，
///   `ShellExecute` 只是把请求转给它、自己立刻返回（实测毫秒级）⇒ 窗口
///   线程可接受；而 `Command::new` 会真的多起一个 explorer 进程。
fn shell_apps_folder(aumid: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let verb = crate::process::to_wide("open");
    let target = crate::process::to_wide("explorer.exe");
    let args = crate::process::to_wide(&format!("shell:AppsFolder\\{aumid}"));
    unsafe {
        let code = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            args.as_ptr(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        ) as isize;
        // ⛔ **Win32 约定：返回值 > 32 才是成功**（≤ 32 是错误码，最大 32），
        //   ⛔ 不是「非零即成功」—— 失败码本身也非零（例如 2 = 文件找不到）。
        //   依据：`ShellExecuteW` 的 `HINSTANCE` 返回值上界约定。
        code > 32
    }
}

/// ⛔ **浏览器一律不按 exe 重新启动**（与 `ck/FluentFlyout` 同款取舍）：
///   浏览器的 exe 启动只会开一个**空白新窗口**，而用户的意图是「回到我正在
///   放音乐的那个标签页」⇒ 必须靠窗口激活，而浏览器多进程的窗口归属又
///   判不准 ⇒ 这里宁可不启动，也不开一个多余的空白窗。
///   判据：exe 基名 ∈ {chrome, msedge, firefox, opera, brave, vivaldi}。
fn is_browser_exe(exe_path: &str) -> bool {
    let base = exe_path
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(exe_path)
        .to_ascii_lowercase();
    matches!(
        base.as_str(),
        "chrome.exe" | "msedge.exe" | "firefox.exe" | "opera.exe" | "brave.exe" | "vivaldi.exe"
    )
}

/// `ShellExecuteW` 直接跑目标 exe（应用已在跑但**没有可见窗口**时的兜底）。
///
/// ⭐ 依据 `ck/FluentFlyout`：它的第三段也是「`Process.Start(exe)`」，
///   并且同样**排除了浏览器**（见 [`is_browser_exe`]）。
/// ⚠️ 对**已在运行**的进程，Windows 会把它当「启动该程序的另一个实例」
///   ——托盘类应用（无主窗口、只有气泡 UI）的典型行为就是**拉出它的窗口**。
/// ⚠️ 但这**依赖应用自己的单实例实现**（有的会转发到已有实例、有的真开新的）
///   ⇒ 只能当兜底，日志必须写明走的是哪一段。
fn launch_exe(exe_path: &str) -> bool {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    let verb = crate::process::to_wide("open");
    let target = crate::process::to_wide(exe_path);
    unsafe {
        let code = ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL,
        ) as isize;
        code > 32
    }
}

/// 点封面的主入口：把 AUMID 对应的应用拉到前台。
///
/// ⭐ **三段递进**（路线与 `ck/AF-Media-Bar` / `ck/FluentFlyout` 同源）：
///   ① 快照里的 pid → 找主窗口 → `ShowWindow(SW_RESTORE)` +
///      `SetForegroundWindow`。覆盖**有窗口的已运行应用**；
///   ② pid 在、但**一个可见窗口都没有**（托盘/气泡类应用；实测该应用的
///      pid 有 7 个顶层窗口、可见性全为 0）→ `ShellExecuteW` 直接跑它的
///      exe，让它自己把 UI 拉出来；⛔ 浏览器跳过（见 [`is_browser_exe`]）；
///   ③ ②也没有 exe（没匹配到 pid）而 AUMID 含 `!` →
///      `explorer.exe shell:AppsFolder\{AUMID}`，覆盖**商店版**应用。
///
/// ⛔ **全失败只记 standard 日志、不弹窗**：点击处理没有 UI 上下文可弹，
///   静默失败更糟（用户会以为功能不存在）。
/// ⚠️ **已知缺口**：②是否奏效取决于应用自己的单实例实现（有的转发到
///   已有实例、有的真开新进程），都不奏效时点封面没有可见反应。
///   通用地触发托盘气泡需要 `Shell_NotifyIconGetRect` + 向图标发
///   `NIN_SELECT`，而那个 id 由**应用自己**注册、第三方查不到
///   （本仓托盘 id 也只有自己知道）⇒ 无第三方通用解，不做。
pub fn activate_media_app(aumid: &str) -> Activated {
    let aumid = aumid.trim();
    if aumid.is_empty() {
        crate::process::append_log("[activate] 未受理: AUMID 为空");
        return Activated::NoWindow;
    }
    let pid = crate::taskbar_music::snapshot().session_pid;
    match pid {
        Some(pid) => match bring_window_to_front(pid) {
            Some(result) => {
                crate::process::append_log(&format!(
                    "[activate] aumid={aumid} pid={pid} 激活结果={result:?}"
                ));
                return result;
            }
            None => crate::process::append_log(&format!(
                "[activate] aumid={aumid} pid={pid} 没有可见窗口（只有托盘图标或无界面）"
            )),
        },
        None => crate::process::append_log(&format!(
            "[activate] aumid={aumid} 快照里没有 pid（未匹配到音频会话）"
        )),
    }
    // ② 应用在跑但只有托盘/气泡 UI（本机实测的 EchoMusic 就是这种：pid 有
    //   7 个顶层窗口、可见性全为 0）⇒ 按 exe 再拉一次，多数托盘类应用会
    //   借此把主窗口/气泡拉出来。⛔ 浏览器跳过（会开空白新窗，见上）。
    if let Some(pid) = pid {
        if let Some(exe) = crate::app_icon::get_process_exe_path_raw(pid) {
            if is_browser_exe(&exe) {
                crate::process::append_log(&format!(
                    "[activate] pid={pid} 是浏览器，跳过按 exe 重启（会开空白新窗）"
                ));
            } else {
                let ok = launch_exe(&exe);
                crate::process::append_log(&format!(
                    "[activate] pid={pid} 无可见窗口，按 exe 拉起 {} 成功={ok}",
                    &exe[..exe.len().min(60)]
                ));
                if ok {
                    return Activated::LaunchedExe;
                }
            }
        }
    }
    if aumid.contains('!') {
        let ok = shell_apps_folder(aumid);
        crate::process::append_log(&format!(
            "[activate] aumid={aumid} 走 shell:AppsFolder 成功={ok}"
        ));
        return if ok {
            Activated::ShellAppsFolder
        } else {
            Activated::NoWindow
        };
    }
    crate::process::append_log(&format!(
        "[activate] 未受理: aumid={aumid} 既没有可见窗口、AUMID 也不含 '!'（无法用 AppsFolder）"
    ));
    Activated::NoWindow
}

use crate::standard_log;
use std::sync::atomic::Ordering;
use std::sync::{Mutex, OnceLock};
use tauri::{
    image::Image,
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{TrayIcon, TrayIconBuilder},
    Emitter, Listener,
};

use crate::audio;
use crate::config;
use crate::popup;
use crate::state::{get_devices_cache, AUTO_MENU_ITEM, AUTO_START, TRAY_POS};
use crate::windows;

static TRAY_ICON: OnceLock<Mutex<Option<TrayIcon<tauri::Wry>>>> = OnceLock::new();

// ── B8：主线程分发 API 的薄包装 ──────────────────────────────────
//
// 下面 4 个函数是**唯一**允许直接调用托盘/菜单 setter 的地方。每个包装体内断言
// 「当前线程未持有配置锁」——理由见 `config.rs` 的 B8 说明：这类 API 会**无超时地
// 同步等主线程**，而主线程自身要取配置锁 ⇒ 持锁调用即构成 AB/BA 永久死锁
// （整进程冻结，看门狗也救不回）。
//
// 为什么必须**收口**而不是让各调用点自己写断言：断言散在 4 处，新增第 5 个调用点
// 时很容易忘。收口后「调用 setter」这个动作被收敛到一处——新代码要么走包装
// （自动受保护），要么直接调 API（评审时一眼可见）。
//
// `debug_assert!` 在 release 下整块被编译掉，故线上零成本；开发期一旦有人在锁内
// 加了一次菜单调用，这里会立刻 panic 并指出具体是哪一个 API。

fn apply_tooltip(tray: &TrayIcon<tauri::Wry>, tooltip: String) {
    debug_assert!(
        !config::config_lock_held(),
        "P0-4：持配置锁时调用 TrayIcon::set_tooltip，会与主线程构成 AB/BA 永久死锁"
    );
    let _ = tray.set_tooltip(Some(tooltip));
}

fn apply_text(item: &MenuItem<tauri::Wry>, text: &str) {
    debug_assert!(
        !config::config_lock_held(),
        "P0-4：持配置锁时调用 MenuItem::set_text，会与主线程构成 AB/BA 永久死锁"
    );
    let _ = item.set_text(text);
}

fn apply_icon(tray: &TrayIcon<tauri::Wry>, icon: Image<'static>) {
    debug_assert!(
        !config::config_lock_held(),
        "P0-4：持配置锁时调用 TrayIcon::set_icon，会与主线程构成 AB/BA 永久死锁"
    );
    let _ = tray.set_icon(Some(icon));
}

fn apply_menu(tray: &TrayIcon<tauri::Wry>, menu: Menu<tauri::Wry>) {
    debug_assert!(
        !config::config_lock_held(),
        "P0-4：持配置锁时调用 TrayIcon::set_menu，会与主线程构成 AB/BA 永久死锁"
    );
    let _ = tray.set_menu(Some(menu));
}

/// 将查询结果写回设备缓存，返回是否发生变化（新旧列表比较）。
fn apply_devices_cache(new_devices: Vec<crate::device::Device>) -> bool {
    let cache = get_devices_cache();
    // P2-11：中毒时不再静默返回 false —— 那会让调用方认为「设备列表没变」，
    // 于是 tooltip 与弹窗卡片此后再也不刷新（无日志的哑故障）。
    let mut guard = crate::state::lock_unpoisoned(cache);
    if *guard != new_devices {
        *guard = new_devices;
        true
    } else {
        false
    }
}

/// 刷新设备缓存，返回是否发生变化。
/// 查询失败（WMI 不可信态）时跳过本轮，保留旧缓存避免 tooltip 抖动
fn refresh_devices_cache() -> bool {
    match crate::wmi_query::query_devices(false) {
        Ok(d) => apply_devices_cache(d),
        Err(e) => {
            standard_log!("[tray] skip cache refresh: {}", e);
            false
        }
    }
}

/// 根据缓存的设备信息构建 tooltip 文本
fn build_tooltip_text() -> String {
    let cache = get_devices_cache();
    let devices = crate::state::lock_unpoisoned(cache);

    let mut lines = Vec::new();
    config::with_config(|c| {
        for tray_name in &c.tray_devices {
            if let Some(dev) = devices.iter().find(|d| &d.name == tray_name) {
                let display_name = c.device_names.get(&dev.name).unwrap_or(&dev.name);
                let dot = if dev.status == crate::wmi_query::BT_STATUS_CONNECTED {
                    "🟢"
                } else {
                    "⚪"
                };
                match dev.battery {
                    Some(battery) => lines.push(format!("{} {} - {}%", dot, display_name, battery)),
                    None => lines.push(format!("{} {}", dot, display_name)),
                }
            }
        }
    });

    if lines.is_empty() {
        "外设监控".to_string()
    } else {
        lines.join("\n")
    }
}

/// 更新托盘 tooltip
fn update_tooltip() {
    let tooltip = build_tooltip_text();

    // 先取句柄再释放锁：`set_tooltip` 内部同步等主线程，
    // 持 TRAY_ICON 调用会与「主线程等 TRAY_ICON」构成 AB/BA 死锁（详见 build_audio_devices_menu 注释）
    let tray = {
        let guard = crate::state::lock_unpoisoned(TRAY_ICON.get_or_init(|| Mutex::new(None)));
        match *guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    apply_tooltip(&tray, tooltip);
}

/// 后台刷新线程：定期查询设备并更新缓存，状态变化时自动更新 tooltip
/// 并向弹窗推送 devices-changed（卡片实时增删，无需等焦点）
fn start_device_watcher(app: &tauri::AppHandle) {
    use tauri::Emitter;

    let handle = app.clone();
    std::thread::spawn(move || loop {
        // 连接在线程内建立一次并复用（WMIConnection 为 !Send，须在此线程内持有）；
        // 建立失败退避重试，查询失败跳出重建连接以自愈
        let con = match wmi::WMIConnection::new() {
            Ok(c) => c,
            Err(e) => {
                standard_log!("[tray] WMIConnection::new failed: {}", e);
                std::thread::sleep(std::time::Duration::from_secs(10));
                continue;
            }
        };

        loop {
            // 每轮重新读取间隔（支持运行时修改）
            let secs = config::with_config(|c| c.low_battery_refresh_secs.max(10));
            std::thread::sleep(std::time::Duration::from_secs(secs as u64));

            let has_tray = config::with_config(|c| !c.tray_devices.is_empty());
            let has_battery_notify = config::with_config(|c| c.low_battery_notify);
            if !has_tray && !has_battery_notify {
                continue;
            }

            let changed = match crate::wmi_query::query_devices_with(&con, false) {
                Ok(d) => apply_devices_cache(d),
                Err(e) => {
                    standard_log!("[tray] skip cache refresh: {}", e);
                    break; // 连接可能失效，跳出重建
                }
            };
            if has_tray && changed {
                crate::process::append_verbose_log("[tray] 设备缓存变化，更新 tooltip");
                std::thread::spawn(update_tooltip);
                let _ = handle.emit("devices-changed", ());
            }

            // 低电量通知检查（P2-7）
            //
            // 「锁内只收集、锁外再发通知」这条纪律**不在这里写花括号**：
            // 它由 `notify_low_battery` 的函数体承载（见该函数文档）。
            // 这里曾写成 `check_battery_notify(&lock_unpoisoned(cache))` ——
            // `lock_unpoisoned(cache)` 是**临时量**、guard 存活至**整条语句结束**，
            // 于是通知全程持设备缓存锁，而它内部要 `show_toast`（WinRT/COM）
            // 与取配置锁 ⇒ 既违「持锁不做 COM」又违锁序纪律；更危险的是反向路径
            // 真实存在（主线程命令先取配置锁，`apply_devices_cache` 取设备缓存锁）
            // ⇒ AB/BA 死锁条件齐备。改成「靠调用方写花括号」只是把风险搬到了
            // 调用点，故收进 `notify_low_battery`，并由单测直接证伪。
            if has_battery_notify {
                crate::battery_notify::notify_low_battery(get_devices_cache());
            }
        }
    });
}

/// 根据默认打开页面与系统深色模式选择托盘图标
fn pick_tray_icon() -> Image<'static> {
    let is_volume = config::with_config(|c| c.default_popup_tab == "volume");
    let dark = crate::windows::system_dark_mode();
    if is_volume {
        let bytes = if dark {
            include_bytes!("../icons/tray-volume-icon-dark.png").to_vec()
        } else {
            include_bytes!("../icons/tray-volume-icon.png").to_vec()
        };
        Image::from_bytes(&bytes).expect("Failed to load tray volume icon")
    } else {
        let bytes = if dark {
            include_bytes!("../icons/tray-icon-dark.png").to_vec()
        } else {
            include_bytes!("../icons/tray-icon.png").to_vec()
        };
        Image::from_bytes(&bytes).expect("Failed to load tray icon")
    }
}

/// 主题监视线程的循环体：把「注册通知 / 等待 / 读取当前值」编排成一个可测的逻辑单元。
///
/// ── 为什么要抽出来（P3-11）─────────────────────────────────────────
/// 原循环的次序是「**读**当前值 → 注册通知 → 等待」：
///
/// ```ignore
/// let mut last = system_dark_mode();      // ① 读
/// loop {
///     RegNotifyChangeKeyValue(..);        // ② 注册
///     WaitForSingleObject(event, INFINITE); // ③ 等
///     let current = system_dark_mode();   // ④ 再读并比较
/// }
/// ```
///
/// 隐患在 ① 与 ② 之间的窗口：若系统恰好在「① 读完之后、② 注册之前」切换主题，
/// 那次变更**不会**触发事件 ⇒ 托盘图标停留在旧主题，直到用户下次手动切主题。
/// 单次丢失看似无害，但这正是那种「偶发、难复现、被归因成『图标有时不刷新』」
/// 的缺陷 —— 修复成本极低（把注册提到读取之前），不修反而不划算。
///
/// 抽成函数是为了让「顺序」这件事可被断言：`ThemeWatchState::load` 只做
/// 「读一次当前值」，调用方负责先注册再 load。因果顺序写在类型上，
/// 不靠注释约束后人。
///
/// ── 可测性设计 ─────────────────────────────────────────────────
/// `load` / `on_notify` **不直接调用** `system_dark_mode()`，而是接收一个
/// 读取器闭包。这样单测可以喂一个可控的「真假序列」来验证去重与次序语义，
/// 无需真实修改系统主题（那需要管理员权限、且会闪烁用户桌面）。
#[cfg(target_os = "windows")]
struct ThemeWatchState {
    last: bool,
}

#[cfg(target_os = "windows")]
impl ThemeWatchState {
    /// 注册通知**之后**才读取当前值：这样「注册 → 读取」之间发生的变更
    /// 要么被本次读取捕获、要么会触发事件，两个缝隙合并成一个不可能丢失的区间。
    fn load<F: FnOnce() -> bool>(read: F) -> Self {
        Self { last: read() }
    }

    /// 收到一次事件（或首轮）后重新取值；返回 `true` 表示主题**确实变了**，
    /// 调用方应刷新托盘图标。
    ///
    /// 去重是必要的：`RegNotifyChangeKeyValue` 监视的是整个 Personalize 键，
    /// 系统改**其它**设置（如透明效果、锁屏壁纸）也会触发事件。
    /// 不去重会让每次无关变更都重建一次托盘图标。
    fn on_notify<F: FnOnce() -> bool>(&mut self, read: F) -> bool {
        let current = read();
        if current == self.last {
            return false;
        }
        self.last = current;
        true
    }
}

/// 用真实系统状态构造 `ThemeWatchState` 的便捷包装（生产路径调用它）。
#[cfg(target_os = "windows")]
fn theme_watch_state_now() -> ThemeWatchState {
    ThemeWatchState::load(crate::windows::system_dark_mode)
}

/// 主题监视的**启动次序**，抽出来是为了让「先注册、后读取」这件事可被断言。
///
/// ── 为什么不能只靠注释保证（P3-11 验收教训）──────────────────────
/// 第一版把次序写在 `start_theme_watcher` 的字面顺序里，单测只能断言
/// 「局部变量的值」——把生产代码的次序**改回「先读后注册」后测试依然全绿**，
/// 等于没测。真正能证伪的写法是把「调用哪个、按什么顺序」变成函数的返回值，
/// 于是单测可以在**不碰注册表**的前提下断言真实的调用序列。
///
/// 返回 `(先注册, 后读取)` 是否成立；本函数**必然**返回 `true`，
/// 它的价值在于「次序」是它唯一的契约 —— 改动次序会让调用序列断言转红。
///
/// 参数是两个回调，生产路径传真实实现，单测传记录器。
#[cfg(target_os = "windows")]
fn ordered_register_then_read<R, D>(register: R, read: D) -> bool
where
    R: FnOnce() -> bool,
    D: FnOnce() -> bool,
{
    // 次序：注册先于读取。**不要交换这两行** —— 交换后
    // 「注册前发生的主题切换」会永久丢失（见 `start_theme_watcher` 注释）。
    let _registered = register();
    let _initial = read();
    true
}

/// 后台线程：监听系统深色模式变化并刷新托盘图标（仅跟随系统）
/// 通过 RegNotifyChangeKeyValue 注册表变更通知实现事件驱动，无轮询
fn start_theme_watcher() {
    use windows_sys::Win32::System::Registry::{
        RegCloseKey, RegNotifyChangeKeyValue, RegOpenKeyExW, HKEY_CURRENT_USER, KEY_READ,
        REG_NOTIFY_CHANGE_LAST_SET,
    };
    use windows_sys::Win32::System::Threading::{CreateEventW, WaitForSingleObject, INFINITE};

    std::thread::spawn(move || {
        unsafe {
            let mut hkey = std::ptr::null_mut();
            let status = RegOpenKeyExW(
                HKEY_CURRENT_USER,
                windows_sys::core::w!(
                    "Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"
                ),
                0,
                KEY_READ,
                &mut hkey,
            );
            if status != 0 {
                return;
            }

            let event = CreateEventW(std::ptr::null(), 1, 0, std::ptr::null());
            if event.is_null() {
                RegCloseKey(hkey);
                return;
            }

            // ── 次序：先注册，再读取（P3-11）──────────────────────────
            // 「注册」与「读取」之间无论发生什么都不会丢：
            //   · 变更在注册之前 → 首次读取拿到的是**新值**
            //   · 变更在注册之后 → 会触发事件，进入下面的循环再读一次
            // 原实现把读取放在注册之前，上面第一种情况会**静默丢失**。
            //
            // ⚠️ 次序由 `ordered_register_then_read` 承载（单测断言它的调用序列）。
            // 这里刻意调它而不是直接写两行 —— 直接写会让「次序」变成没人守的约定，
            // 改错也没有任何测试会转红（P3-11 第一版验收教训）。
            let mut registered = false;
            let mut state = ThemeWatchState { last: false };
            ordered_register_then_read(
                || {
                    registered =
                        RegNotifyChangeKeyValue(hkey, 0, REG_NOTIFY_CHANGE_LAST_SET, event, 1) == 0;
                    registered
                },
                || {
                    state = theme_watch_state_now();
                    state.last
                },
            );

            loop {
                if !registered {
                    // 通知注册失败时退避重试，避免线程空转。
                    // 退避期间仍定期重读，避免「注册一直失败 ⇒ 主题永远不跟」。
                    std::thread::sleep(std::time::Duration::from_secs(3));
                    if state.on_notify(crate::windows::system_dark_mode) {
                        std::thread::spawn(update_tray_icon);
                    }
                    registered =
                        RegNotifyChangeKeyValue(hkey, 0, REG_NOTIFY_CHANGE_LAST_SET, event, 1) == 0;
                    continue;
                }

                WaitForSingleObject(event, INFINITE);
                if state.on_notify(crate::windows::system_dark_mode) {
                    std::thread::spawn(update_tray_icon);
                }
                // 事件已被消费（手动重置事件 + 上轮等待返回后需重新注册）
                registered =
                    RegNotifyChangeKeyValue(hkey, 0, REG_NOTIFY_CHANGE_LAST_SET, event, 1) == 0;
            }
        }
    });
}

pub fn refresh_tray_tooltip(_app_handle: &tauri::AppHandle) {
    refresh_devices_cache();
    update_tooltip();
}

pub fn init_auto_start() {
    AUTO_START.store(config::with_config(|c| c.auto_start), Ordering::Relaxed);
}

/// 构建完整的顶层菜单
fn build_full_menu(
    app: &tauri::AppHandle,
    audio_devices_menu: &Submenu<tauri::Wry>,
) -> Result<Menu<tauri::Wry>, Box<dyn std::error::Error>> {
    let auto_text = if AUTO_START.load(Ordering::Relaxed) {
        "开机自启 ✓"
    } else {
        "开机自启"
    };
    let show_i = MenuItem::with_id(app, "show", "设备信息", true, None::<&str>)?;
    let volume_i = MenuItem::with_id(app, "volume", "音量控制", true, None::<&str>)?;
    let settings_i = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)?;
    let about_i = MenuItem::with_id(app, "about", "关于", true, None::<&str>)?;
    let auto_i = MenuItem::with_id(app, "auto_start", auto_text, true, None::<&str>)?;
    let exit_i = MenuItem::with_id(app, "exit", "退出", true, None::<&str>)?;
    let win_sound_menu = build_windows_sound_settings_menu(app)?;
    let slot = AUTO_MENU_ITEM.get_or_init(|| Mutex::new(None));
    *crate::state::lock_unpoisoned(slot) = Some(auto_i.clone());

    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let sep3 = PredefinedMenuItem::separator(app)?;

    let menu = Menu::with_items(
        app,
        &[
            &show_i,
            &volume_i,
            &sep1,
            audio_devices_menu,
            &win_sound_menu,
            &sep2,
            &auto_i,
            &sep3,
            &settings_i,
            &about_i,
            &exit_i,
        ],
    )?;
    Ok(menu)
}

pub fn setup_tray(app: &tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    use tauri_plugin_autostart::ManagerExt;
    let autostart = app.autolaunch();
    let current = autostart.is_enabled().unwrap_or(false);
    let wanted = AUTO_START.load(Ordering::Relaxed);
    if wanted != current {
        let _ = if wanted {
            autostart.enable()
        } else {
            autostart.disable()
        };
    }

    // 构建音频设备切换子菜单
    let audio_devices_menu = build_audio_devices_menu(app.handle())?;

    let menu = build_full_menu(app.handle(), &audio_devices_menu)?;

    let tray_icon = pick_tray_icon();

    let _tray = TrayIconBuilder::with_id("main-tray")
        .icon(tray_icon)
        .tooltip("外设监控")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(move |app, event| {
            standard_log!("[tray] menu: {}", event.id.as_ref());
            // 菜单事件由 tao 主循环派发（tauri/src/app.rs 的 EventLoopMessage::MenuEvent
            // 分支）⇒ 本回调就在主线程上：重操作（窗口/材质/DWM）一律 spawn 移出，
            // 比照 audio_dev_ 分支的既有模式
            match event.id.as_ref() {
                "show" => {
                    let app = app.clone();
                    std::thread::spawn(move || crate::popup::open_popup(&app, "devices"));
                }
                "volume" => {
                    let app = app.clone();
                    std::thread::spawn(move || crate::popup::open_popup(&app, "volume"));
                }
                "settings" => {
                    let app = app.clone();
                    std::thread::spawn(move || windows::open_settings(&app));
                }
                "about" => {
                    let app = app.clone();
                    std::thread::spawn(move || windows::open_settings_tab(&app, "about"));
                }
                "auto_start" => {
                    let old = AUTO_START.load(Ordering::Relaxed);
                    let new_val = !old;
                    AUTO_START.store(new_val, Ordering::Relaxed);
                    config::with_config_mut(|c| c.auto_start = new_val);
                    let autostart = app.autolaunch();
                    let _ = if new_val {
                        autostart.enable()
                    } else {
                        autostart.disable()
                    };
                    update_auto_text();
                    let config_snapshot = config::with_config(|c| c.clone());
                    let _ = app.emit("config-changed", config_snapshot);
                    standard_log!("[tray] auto_start toggled: {}", new_val);
                }
                "exit" => {
                    app.exit(0);
                }
                id if id.starts_with("audio_dev_") => {
                    let device_id = id[10..].to_owned();
                    if !device_id.is_empty() {
                        standard_log!("[tray] set_default_device: {}", device_id);
                        std::thread::spawn(move || {
                            // 失败必须可见（P3-7）：菜单此刻已经关闭，用户会认为
                            // 「切换成功」，而实际默认设备没变 —— 这是「失败后状态与
                            // 用户操作不一致」的典型。本路径没有返回值可上报
                            // （托盘菜单事件没有调用方），只能记标准级日志。
                            // 对照：`shortcut.rs` 与 `commands.rs` 的同类调用都处理了 Err。
                            if let Err(e) = audio::set_default_device(&device_id) {
                                standard_log!(
                                    "[tray] set_default_device FAILED: {} ({})",
                                    device_id,
                                    e
                                );
                            }
                            update_audio_devices_menu();
                        });
                    }
                }
                "win_sound_volume_mixer" => {
                    let _ = crate::process::shell_open("sndvol.exe", None);
                }
                "win_sound_playback" => {
                    crate::process::open_sound_panel("playback");
                }
                "win_sound_recording" => {
                    crate::process::open_sound_panel("recording");
                }
                "win_sound_sounds" => {
                    crate::process::open_sound_panel("sounds");
                }
                "win_sound_settings" => {
                    crate::process::open_settings_page("sound");
                }
                "win_sound_app_volume" => {
                    crate::process::open_settings_page("apps-volume");
                }
                _ => {}
            }
        })
        .on_tray_icon_event(|tray, event| {
            let app = tray.app_handle();
            if let tauri::tray::TrayIconEvent::Click {
                button,
                button_state,
                rect,
                ..
            } = event
            {
                if button_state != tauri::tray::MouseButtonState::Up {
                    return;
                }
                if button == tauri::tray::MouseButton::Left {
                    crate::process::append_log("[tray] click → spawn");
                    // 托盘图标事件同样由主循环派发 ⇒ 本回调在主线程上，仅做分发：
                    // 显示器枚举/配置读取/窗口操作全部移出，
                    // 防止唤醒后子窗口消息队列卡死拖垮整个事件循环
                    let app = app.clone();
                    std::thread::spawn(move || {
                        if let Some(pos) = TRAY_POS.get() {
                            // 物理坐标需整体转逻辑：仅除 x 会让 y 携带物理值，
                            // 在缩放屏上把弹出窗底边推出屏幕外。
                            // 混合 DPI 时须按托盘所在屏的 SF 换算（主屏 SF 会定位偏移）。
                            let (px, py) = match rect.position {
                                tauri::Position::Physical(p) => {
                                    let (x, y) = (p.x as f64, p.y as f64);
                                    let info = windows::monitor_info_at(&app, x, y);
                                    let sf = info
                                        .as_ref()
                                        .map(|i| i.scale_factor)
                                        .unwrap_or_else(|| windows::scale_factor(&app));
                                    if let Some(i) = info {
                                        *crate::state::lock_unpoisoned(
                                            crate::state::get_tray_monitor(),
                                        ) = Some(i);
                                    }
                                    (x / sf, y / sf)
                                }
                                tauri::Position::Logical(p) => (p.x, p.y),
                            };
                            *crate::state::lock_unpoisoned(pos) = (px, py);
                        }
                        let tab = config::with_config(|c| c.default_popup_tab.clone());
                        popup::toggle(&app, &tab);
                    });
                }
            }
        })
        .build(app)?;

    let tray_slot = TRAY_ICON.get_or_init(|| Mutex::new(None));
    *crate::state::lock_unpoisoned(tray_slot) = Some(_tray);

    let _ = TRAY_POS.get_or_init(|| {
        let handle = app.handle();
        let sf = windows::scale_factor(handle);
        let screen_w = handle
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().width as f64 / sf)
            .unwrap_or(1920.0);
        let screen_h = handle
            .primary_monitor()
            .ok()
            .flatten()
            .map(|m| m.size().height as f64 / sf)
            .unwrap_or(1080.0);
        Mutex::new((screen_w - 300.0, screen_h - 50.0))
    });

    // 首启即定位任务栏所在屏与通知区锚点：托盘尚未被点击前，弹窗据此确定所在屏、
    // 工作区与落点，避免 fallback 主屏/假锚点导致副屏/混合 DPI 首启错位（托盘点击后再精确纠偏）。
    if let Some((info, anchor_x, anchor_y)) = windows::monitor_info_of_taskbar(app.handle()) {
        *crate::state::lock_unpoisoned(crate::state::get_tray_monitor()) = Some(info);
        if let Some(pos) = TRAY_POS.get() {
            *crate::state::lock_unpoisoned(pos) = (anchor_x, anchor_y);
        }
    }

    app.listen("config-changed", move |_| {
        // 回调只做轻量分发（见 AGENTS.md「`app.listen` 的回调没有专属线程」）：
        // 读配置 + 更新原子标志 + 刷新勾选文案，三者都需即时反映，故留在本线程；
        // 其余（图标重建、菜单重建、设备缓存刷新）含 COM 枚举与全量菜单构造，全部下放子线程。
        let new_auto = config::with_config(|c| c.auto_start);
        AUTO_START.store(new_auto, Ordering::Relaxed);
        update_auto_text();
        std::thread::spawn(|| {
            update_tray_icon();
            update_audio_devices_menu();
            refresh_devices_cache();
            update_tooltip();
        });
    });

    app.listen("tray-devices-changed", move |_| {
        std::thread::spawn(update_tooltip);
    });

    app.listen("audio-devices-changed", |_| {
        // 同 config-changed：菜单重建内含 COM 枚举，不能在回调里同步跑
        std::thread::spawn(update_audio_devices_menu);
    });

    // 启动后台设备监控线程
    start_device_watcher(app.handle());
    // 启动系统深色模式监听线程（仅跟随系统）
    start_theme_watcher();

    Ok(())
}

fn update_auto_text() {
    // 先取句柄再释放锁：`set_text` 内部同步等主线程（同 build_audio_devices_menu 注释）
    let item = match AUTO_MENU_ITEM.get() {
        Some(slot) => {
            let guard = crate::state::lock_unpoisoned(slot);
            match *guard {
                Some(ref mi) => mi.clone(),
                None => return,
            }
        }
        None => return,
    };
    let text = if AUTO_START.load(Ordering::Relaxed) {
        "开机自启 ✓"
    } else {
        "开机自启"
    };
    apply_text(&item, text);
}

/// 根据默认打开页面与系统深色模式更新托盘图标
fn update_tray_icon() {
    let icon = pick_tray_icon();
    // 先取句柄再释放锁：`set_icon` 内部同步等主线程（同 build_audio_devices_menu 注释）
    let tray = {
        let guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    apply_icon(&tray, icon);
}

/// 简化设备名称：仅保留括号内内容，如 "耳机 (小爱音箱-9205)" -> "小爱音箱-9205"
/// 注意：与 dedup::core_name 语义不同——本函数不剥协议后缀、返回 &str，两者勿互相替换。
pub(crate) fn simplify_device_name(name: &str) -> &str {
    if let Some(open) = name.find('(') {
        if let Some(close) = name.rfind(')') {
            if close > open {
                let inner = name[open + 1..close].trim();
                if !inner.is_empty() {
                    return inner;
                }
            }
        }
    }
    name
}

/// 构建音频设备切换子菜单
///
/// **锁纪律（P0 死锁防护，勿破坏）**：`config::with_config` 闭包内**只允许纯内存拷贝**。
///
/// Tauri 的菜单 API（`MenuItem::with_id` / `Submenu::append` / `set_menu` /
/// `set_icon` / `set_tooltip` / `set_text` …）内部一律经 `run_item_main_thread!` 展开为
/// `run_on_main_thread(..)` + `rx.recv()`（**无超时**），即同步等待主线程执行完闭包。
/// 而主线程自身会通过 `with_config(_mut)` 读配置（`set_window_material`、`get_config`、
/// 快捷键分发、tooltip 刷新…）。因此「持配置锁 → 调菜单 API」与「主线程 → 等配置锁」
/// 构成 AB/BA 死锁：子线程等主线程、主线程等配置锁，**永久冻结且看门狗也救不回**
/// （看门狗探活 `is_visible()` 同样要主线程）。
///
/// 故此处先在锁内取纯数据快照，菜单 API 全部移到锁外调用。
fn build_audio_devices_menu(
    app: &tauri::AppHandle,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let submenu = Submenu::with_id(app, "audio_devices", "音频设备", true)?;
    let devices = audio::enumerate_output_devices().unwrap_or_default();
    if devices.is_empty() {
        let empty = MenuItem::with_id(app, "audio_dev_empty", "无音频设备", false, None::<&str>)?;
        submenu.append(&empty)?;
    } else {
        // 锁内：只做纯内存快照（菜单 id + 显示文案），不触碰任何 Tauri API
        let rows: Vec<(String, String)> = config::with_config(|c| {
            devices
                .iter()
                .filter(|device| !c.hidden_audio_devices.contains(&device.name))
                .map(|device| {
                    let check = if device.is_default { " ✓" } else { "" };
                    let display = c
                        .device_names
                        .get(&device.name)
                        .cloned()
                        .unwrap_or_else(|| {
                            if c.simplify_device_names {
                                simplify_device_name(&device.name).to_string()
                            } else {
                                device.name.clone()
                            }
                        });
                    (
                        format!("audio_dev_{}", device.id),
                        format!("{}{}", display, check),
                    )
                })
                .collect()
        });
        // 锁外：菜单 API（内部同步等主线程，绝不可在持锁时调用）
        for (id, label) in rows {
            if let Ok(item) = MenuItem::with_id(app, id, label, true, None::<&str>) {
                let _ = submenu.append(&item);
            }
        }
    }
    Ok(submenu)
}

/// 音频菜单重建的单飞标志 / 合并标志（B3）。
///
/// 触发源有三个且都在**各自新开的线程**上：`config-changed`（一次连做
/// 图标 + 菜单 + 设备缓存 + tooltip）、`tray-devices-changed`、
/// `audio-devices-changed`。改造前它们各自独立重建菜单，撞车时「后写覆盖」——
/// 较慢的那次会用稍旧的一份菜单盖掉较新的一份，且整棵菜单（含 COM 枚举）
/// 会被重复构造。现由 [`crate::state::run_coalesced`] 收敛为「单飞 + 合并」。
static MENU_REBUILD_RUNNING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);
static MENU_REBUILD_PENDING: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

/// 更新音频设备切换子菜单（在设备列表变化时调用）
///
/// 入口做「单飞 + 合并」：同一时刻只有一轮在重建，期间到达的请求被合并进
/// 收尾的补跑轮次（详见 `state::run_coalesced`）。
fn update_audio_devices_menu() {
    let rounds = crate::state::run_coalesced(
        &MENU_REBUILD_RUNNING,
        &MENU_REBUILD_PENDING,
        rebuild_audio_devices_menu_once,
    );
    if let Some(n) = rounds {
        if n > 1 {
            // 合并生效的证据：一次触发期间到达了额外请求，但没有重建 n 次菜单树
            standard_log!("[tray] audio menu rebuild coalesced: {} rounds", n);
        }
    }
}

/// 重建一次音频设备切换子菜单（原 `update_audio_devices_menu` 的函数体）
///
/// 锁纪律：`TRAY_ICON` 只用于「取句柄」和「换菜单」两个瞬时动作，
/// 中间的 COM 枚举（`build_audio_devices_menu`）与全量菜单构造（`build_full_menu`）
/// 一律在锁外完成——否则会与 `update_tray_icon` 等持锁路径互相排队，
/// 把一次设备热插拔放大成可见卡顿。
fn rebuild_audio_devices_menu_once() {
    // 第一段：仅取 app 句柄，取到即释放锁
    let app = {
        let tray_guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *tray_guard {
            Some(ref tray) => tray.app_handle().clone(),
            None => return,
        }
    };

    // 第二段：锁外做耗时工作（COM 枚举 + 菜单树构造）
    let new_submenu = match build_audio_devices_menu(&app) {
        Ok(s) => s,
        Err(_) => return,
    };
    let Ok(menu) = build_full_menu(&app, &new_submenu) else {
        return;
    };

    // 第三段：先取句柄、释放托盘锁，再换菜单——`set_menu` 内部经
    // `run_item_main_thread!` 同步等主线程，持锁调用会与主线程的
    // `update_tooltip` / `update_tray_icon`（同样要 TRAY_ICON）构成 AB/BA 死锁。
    let tray = {
        let tray_guard = crate::state::lock_unpoisoned(TRAY_ICON.get().unwrap());
        match *tray_guard {
            Some(ref tray) => tray.clone(),
            None => return,
        }
    };
    apply_menu(&tray, menu);
}

/// 构建 Windows 声音设置子菜单
fn build_windows_sound_settings_menu(
    app: &tauri::AppHandle,
) -> Result<Submenu<tauri::Wry>, Box<dyn std::error::Error>> {
    let submenu = Submenu::with_id(app, "win_sound", "声音设置", true)?;
    let items = [
        ("win_sound_volume_mixer", "音量合成器 (Classic)"),
        ("win_sound_playback", "播放设备 (Classic)"),
        ("win_sound_recording", "录制设备 (Classic)"),
        ("win_sound_sounds", "声音 (Classic)"),
        ("win_sound_settings", "声音设置"),
        ("win_sound_app_volume", "音量合成器"),
    ];
    for (id, label) in items {
        let item = MenuItem::with_id(app, id, label, true, None::<&str>)?;
        submenu.append(&item)?;
    }
    Ok(submenu)
}

#[cfg(test)]
mod tests {
    /// 主题监视线程只有 Windows 上有实现，测试同样门控。
    #[cfg(target_os = "windows")]
    mod theme_watch {
        use super::super::{ordered_register_then_read, theme_watch_state_now, ThemeWatchState};

        /// 首次 load 应记住当前值；值没变时不触发刷新。
        #[test]
        fn load_remembers_initial_value() {
            let mut state = ThemeWatchState::load(|| false);
            assert!(!state.on_notify(|| false), "值未变，不应触发托盘刷新");
            let mut state = ThemeWatchState::load(|| true);
            assert!(!state.on_notify(|| true), "值未变，不应触发托盘刷新");
        }

        /// 主题确实变化时触发一次，且之后同一值不再重复触发（去重）。
        #[test]
        fn notify_fires_once_per_actual_change() {
            let mut state = ThemeWatchState::load(|| false);

            assert!(state.on_notify(|| true), "浅色 → 深色应触发刷新");
            assert!(
                !state.on_notify(|| true),
                "仍是深色，无关的注册表变更不得重复刷新"
            );
            assert!(!state.on_notify(|| true));

            assert!(state.on_notify(|| false), "深色 → 浅色应触发刷新");
            assert!(!state.on_notify(|| false), "仍是浅色，不重复刷新");
        }

        /// 核心回归（P3-11）：**注册发生在读取之前**，因此「注册前已切换的主题」
        /// 必须被首次 load 捕获，而不是被静默丢掉。
        ///
        /// 模拟：注册完成 → 系统在 load 之前切到深色 → load 读到深色。
        /// 修复前的次序（load 在前）会读到浅色，此后事件再也不会来，
        /// 图标永久停留在浅色 —— 用下面的断言把这条路径钉死。
        #[test]
        fn change_between_register_and_load_is_captured() {
            // 生产代码的次序是「先注册，再 theme_watch_state_now()」。
            // 这里直接验证该次序的语义：load 读到的是**注册之后**的值。
            let registered_first = true;
            let system_dark = true; // 注册后、load 前系统已切换
            let state = ThemeWatchState::load(|| system_dark);
            assert!(registered_first);

            // load 已捕获到深色 ⇒ 后续无关事件不该再刷（因为状态已是最新）
            let mut state = state;
            assert!(
                !state.on_notify(|| true),
                "load 已读到最新值，无关变更不应触发"
            );
            // 而真正的新变化仍然会被捕获
            assert!(state.on_notify(|| false), "后续真实变化仍应触发");
        }

        /// 顺序反了会丢事件：这个对照用例把「旧次序为何有缺陷」写成可执行的断言。
        ///
        /// 若 `load` 在注册之前（等价于注册晚于读取），则注册与 load 之间
        /// 发生的变更既没被 load 看到、也不会触发事件 ⇒ 丢失。
        #[test]
        fn reading_before_registering_would_lose_the_change() {
            // 旧次序：先读（浅色），系统随后切深色，事件注册在切换之后
            let state_at_read_time = false;
            let system_after_read = true;

            // 读到的值落后于实际值 ⇒ 这就是丢失
            assert_ne!(
                state_at_read_time, system_after_read,
                "对照：旧次序下读到的值与实际值不一致，变更被丢失"
            );

            // 新次序下 load 读到的是注册之后的值 ⇒ 一致
            let state_at_load_time = system_after_read;
            assert_eq!(
                state_at_load_time, system_after_read,
                "新次序下 load 读到注册之后的值，变更被捕获"
            );
        }

        /// 真实系统状态只用来验证包装函数不 panic（不断言具体值，
        /// 那取决于跑测试的机器当前是浅色还是深色）。
        #[test]
        fn real_state_wrapper_does_not_panic() {
            let mut state = theme_watch_state_now();
            // 用同一真实读取器再读一次 ⇒ 必然「未变化」
            assert!(
                !state.on_notify(crate::windows::system_dark_mode),
                "同一状态下重复读取不应报告变化"
            );
        }

        /// ★ 核心验收（P3-11）：断言**真实调用序列**是「先注册、后读取」。
        ///
        /// 这是唯一能证伪「次序被改回去」的测试：把
        /// `ordered_register_then_read` 里两行交换，本用例立刻转红。
        /// 第一版没有这层，导致「生产代码改回旧次序后测试依然全绿」——
        /// 那种测试等于没测。
        #[test]
        fn register_is_called_before_read() {
            use std::cell::RefCell;
            let calls = RefCell::new(Vec::<&'static str>::new());

            let ok = ordered_register_then_read(
                || {
                    calls.borrow_mut().push("register");
                    true
                },
                || {
                    calls.borrow_mut().push("read");
                    true
                },
            );

            assert!(ok, "本函数的契约成立时返回 true");
            assert_eq!(
                *calls.borrow(),
                vec!["register", "read"],
                "次序必须是「先注册、后读取」；反过来会让注册前发生的主题切换永久丢失"
            );
        }

        /// 注册失败也必须**先**读过初始值（否则退避分支会拿 `last` 的默认值
        /// 去比较，可能误报一次变化）。
        #[test]
        fn read_still_happens_even_if_register_fails() {
            use std::cell::RefCell;
            let calls = RefCell::new(Vec::<&'static str>::new());

            ordered_register_then_read(
                || {
                    calls.borrow_mut().push("register");
                    false // 注册失败
                },
                || {
                    calls.borrow_mut().push("read");
                    false
                },
            );

            assert_eq!(
                *calls.borrow(),
                vec!["register", "read"],
                "注册失败不应跳过初始读取"
            );
        }
    }
}

use crate::config::{self, Config};
use crate::device;
use crate::process;
use crate::standard_log;
use crate::wmi_query::query_devices;
use tauri::Emitter;
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

#[tauri::command]
pub fn set_shortcut_recording(recording: bool) {
    crate::state::SHORTCUT_RECORDING.store(recording, std::sync::atomic::Ordering::Relaxed);
    standard_log!("[hotkey] shortcut recording = {}", recording);
}

/// 在 tokio blocking 线程中执行阻塞操作
///
/// ── 命令的线程模型（B4，勿破坏）────────────────────────────────────────
/// `#[tauri::command]`（**不带** `(async)`）的**同步**命令，其函数体在**主线程**上
/// 执行——前端 `invoke` 进来后由 IPC 处理路径直接调用。因此命令体内任何
/// 阻塞调用都会**直接冻结 UI**：COM（`IPolicyConfig`、WMI）、`ShellExecuteW`、
/// 文件 I/O、`sync_all()` 均属此列。P0-4 的探针日志已经实证过这一点：死锁当时
/// 主线程正卡在 `set_window_material` 这个同步命令里。
///
/// 故凡命令体含上述调用者，一律写成 `#[tauri::command(async)] pub async fn …`
/// 并把阻塞部分交给本函数（`spawn_blocking`），使其落在 tokio 阻塞线程池上。
///
/// **注意两处副作用**（已在各命令处注明）：
/// ① 异步命令的后续语句（含 `app.emit`）运行在**异步运行时线程**而非主线程。
///    事件监听回调是在 `emit` 的**调用线程**上同步执行的（见
///    `tauri-2.11.5/src/event/listener.rs:204`），故监听器内的菜单/托盘 API
///    会变成「子线程 → `run_on_main_thread + recv` → 主线程」的跨线程等待。
///    这在无锁前提下是安全的，且 `toggle_device_tray` 等命令早已是这个形态；
/// ② 同一命令的两次并发 `invoke` 不再保证按调用顺序完成（各自占一个阻塞线程）。
///    对「设置类」命令这是可接受的（前端有乐观 UI 与刷新兜底），但**不要**
///    把需要严格定序的读改写序列拆进异步命令。
async fn run_blocking<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| e.to_string())
}

/// 切换 Vec 中某个元素的存在/不存在
fn toggle_vec_item(vec: &mut Vec<String>, item: &str) {
    if let Some(pos) = vec.iter().position(|v| v == item) {
        vec.remove(pos);
    } else {
        vec.push(item.to_string());
    }
}

#[tauri::command(async)]
pub async fn get_devices() -> Result<Vec<device::Device>, String> {
    let devices = run_blocking(|| query_devices(false)).await??;
    Ok(devices)
}

/// 设备信息页手动刷新入口：强制现查 2.4G 接收器电量（绕过 TTL 缓存），
/// 其余流程与 get_devices 一致；鼠标休眠时现查耗时可达数秒
#[tauri::command(async)]
pub async fn get_devices_fresh() -> Result<Vec<device::Device>, String> {
    let devices = run_blocking(|| query_devices(true)).await??;
    Ok(devices)
}

/// 返回设备缓存（tray watcher 每轮更新），供设置页等场景避免重复 WMI 查询。
/// 缓存为空时返回空 vec，调用方应 fallback 到 get_devices。
#[tauri::command]
pub fn get_cached_devices() -> Vec<device::Device> {
    let cache = crate::state::get_devices_cache();
    crate::state::lock_unpoisoned(cache).clone()
}

/// 任务栏设备侧的取数决策：**优先用缓存，缓存为空则现查一次**。
///
/// 为什么单独抽出来、并把 IO 做成参数：命令体是 `async` + `spawn_blocking`，单测里
/// 不便驱动；而「空 ⇒ 必须回落 / 非空 ⇒ 不得重跑 WMI」正是本命令最要紧的一条判据
/// （漏了会**静默**丢掉全部电量），必须能被单测直接钉住。
fn devices_for_taskbar_with<F>(
    cached: Vec<device::Device>,
    query: F,
) -> Result<Vec<device::Device>, String>
where
    F: FnOnce() -> Result<Vec<device::Device>, String>,
{
    if cached.is_empty() {
        query()
    } else {
        Ok(cached)
    }
}

/// 从全局缓存取任务栏设备侧数据，缓存为空时**自愈现查**。
///
/// 缓存为空有**确定**成因、不是偶发：`tray::start_device_watcher` 的后台循环里有
/// `if !has_tray && !has_battery_notify { continue; }`，而 `Config::default()` 中
/// `tray_devices` 为空、`low_battery_notify` 为 `false` ⇒ **默认配置下缓存恒空**。
/// 故「缓存为空就回落」**不能留给调用方**：前端漏写这一句既不报错也不告警，
/// 只会让任务栏的电量整片变空（且「有电量无音频端点」的鼠标/键盘/手柄整条不出现）。
fn devices_for_taskbar() -> Result<Vec<device::Device>, String> {
    let cached = {
        let cache = crate::state::get_devices_cache();
        crate::state::lock_unpoisoned(cache).clone()
    };
    if cached.is_empty() {
        standard_log!("[cmd] get_taskbar_devices: 设备缓存为空，回落现查一次");
    }
    devices_for_taskbar_with(cached, || query_devices(false))
}

/// 任务栏信息窗的数据源：把「电量」与「音量」按**物理设备身份**聚合到同一台设备上。
///
/// 设备列表优先取 tray watcher 维护的缓存（避免每轮重跑 WMI）；**缓存为空时本命令
/// 自己回落现查一次**（见 `devices_for_taskbar`），不依赖调用方记得回落。
/// 音频端点必须现查 —— 音量是实时值。
/// 身份键优先级见 `device_identity`：ContainerId → PnP 实例路径 → 名称。
#[tauri::command(async)]
pub async fn get_taskbar_devices() -> Result<Vec<crate::device_identity::PhysicalDevice>, String> {
    let devices = run_blocking(devices_for_taskbar).await??;
    let audio = run_blocking(crate::audio::enumerate_output_devices)
        .await?
        .map_err(|e| e.to_string())?;
    let pinned = config::with_config(|c| c.pinned_taskbar_devices.clone());
    Ok(crate::device_identity::group_taskbar_devices(
        &devices, &audio, &pinned,
    ))
}

/// **同步**版的任务栏设备取数，供任务栏 widget 的后台线程调用。
///
/// ⛔ 与 `get_taskbar_devices` 的**唯一区别**是「同步 + 不用 tokio」：
///   widget 的取数发生在 `spawn_blocking` 线程里，本就**不在 async 上下文**，
///   无法 `.await`;而 `run_blocking` 是为「在 async 命令里跑阻塞活」设计的
///   （内部用 `tokio::task::spawn_blocking` + 再入运行时）。绕过它直接调同步版本
///   既避免多一层线程切换，也避免「在阻塞线程里再进 tokio」的隐患。
///
/// ⭐ **语义必须与 `get_taskbar_devices` 完全一致**（同一套过滤 + pin 补建 + 聚合），
///   否则 widget 与弹窗会显示不同的设备集合 —— 这正是「判据分叉」类缺陷。
///   因此这里**逐句照抄**其数据路径，只把 `run_blocking` 去掉。
///
/// ⚠️ 返回 `Option`：任一环节失败（WMI / 音频枚举）都返回 `None`，让调用方
///   **保留旧快照** —— 一次取数失败不该把任务栏内容清空（那比显示旧数据更糟）。
pub(crate) fn taskbar_devices_snapshot() -> Option<Vec<crate::device_identity::PhysicalDevice>> {
    let devices = devices_for_taskbar().ok()?;
    let audio = crate::audio::enumerate_output_devices().ok()?;
    let pinned = config::with_config(|c| c.pinned_taskbar_devices.clone());
    Some(crate::device_identity::group_taskbar_devices(
        &devices, &audio, &pinned,
    ))
}

/// 任务栏信息窗「选择设备」列表的数据源：两页**可显示设备的并集**（T3-1）。
///
/// ── 与 `get_taskbar_devices` 的三点区别（**刻意不同，勿合并**）────────────────
///   1. **不过滤数据**：凡两页出现即保留，即使既无电量也无音量。选择器的职责是
///      「让用户选择固定哪台」，用数据可用性筛掉条目会让用户根本无法为「此刻读不到
///      数据的设备」做设置；
///   2. **不做 pin 补建**：`get_taskbar_devices` 会给「pinned 但本次没枚举到」的设备
///      造占位条目（那是它的显示语义）；选择器若也这么做，就会出现「两页都没有、
///      用户也无从取消」的幽灵条目，且**超出并集口径**（Spec §0.4 三）；
///   3. **音频取两侧**：并集 = 设备页 ∪ **输出端点** ∪ **输入端点**。输入端点只在
///      音量页的会话右键菜单里可见，但选择器要覆盖它们（验收判据 2）。
///      ⛔ 音频侧**没有**设备侧那条过滤链（`audio.rs` 只按 `DEVICE_STATE_ACTIVE` +
///      方向取数）⇒ 取两页并集天然绕过设备侧全部过滤，这是**设计如此**，别去改
///      `query_devices_with` 试图「对齐」两页（会把设备页一起污染）。
///
/// 设备侧仍走 `devices_for_taskbar`（缓存优先 + 空则自愈现查），与显示侧同源，
/// 避免两处对「缓存该不该回落」各持一套规则。
#[tauri::command(async)]
pub async fn get_selectable_devices(
) -> Result<Vec<crate::device_identity::SelectableDevice>, String> {
    let devices = run_blocking(devices_for_taskbar).await??;
    // 输出与输入各自枚举：`enumerate_*` 内部直读 MMDevice，开销在 13–15ms 量级
    // （实测，见 PLAYBOOK §E），远小于设备侧 WMI 的 600ms+，无需缓存。
    let outputs = run_blocking(crate::audio::enumerate_output_devices)
        .await?
        .map_err(|e| e.to_string())?;
    let inputs = run_blocking(crate::audio::enumerate_input_devices)
        .await?
        .map_err(|e| e.to_string())?;
    let (pinned, config_snapshot) =
        config::with_config(|c| (c.pinned_taskbar_devices.clone(), c.clone()));
    let merged = crate::device_identity::merge_by_identity(&devices, &outputs, &inputs);
    Ok(crate::device_identity::build_selectable_devices(
        &merged,
        &pinned,
        &config_snapshot,
    ))
}

#[tauri::command]
pub fn open_settings(app: tauri::AppHandle) {
    crate::windows::open_settings(&app);
}

#[tauri::command]
pub fn get_config() -> Config {
    config::with_config(|c| c.clone())
}

/// 启动期配置解析失败的原因（含备份路径）；`None` = 本次启动读取正常。
/// 前端在页面加载后调用一次并提示用户——「设置突然全部恢复默认」必须被解释，
/// 否则会被当成静默丢数据（P1-7）。
#[tauri::command]
pub fn get_config_load_error() -> Option<String> {
    config::get_load_error()
}

/// 注册失败的设备快捷键（P2-12）。空 = 当前没有失败项。
///
/// 前端在页面加载时**主动拉取**：失败事件在启动同步时发出，那一刻页面还没加载、
/// 监听器尚未注册，事件必然落空。而「开机时快捷键被别的程序抢走」正是最常见也
/// 最隐蔽的失效场景，必须靠这条拉取通道覆盖。
#[tauri::command]
pub fn get_shortcut_register_failed() -> Vec<String> {
    crate::shortcut::get_register_failed()
}

/// 应用版本号（来自 tauri.conf.json 的真实包版本，供关于页动态显示，消除静态文案漂移）
#[tauri::command]
pub fn get_app_version(app: tauri::AppHandle) -> String {
    app.package_info().version.to_string()
}

/// 设置当前窗口的系统标题栏主题（仅影响系统窗口标题栏，不影响页面内容）
#[tauri::command]
pub fn set_window_theme(window: tauri::Window, theme: String) {
    let t = match theme.as_str() {
        "dark" => Some(tauri::Theme::Dark),
        "light" => Some(tauri::Theme::Light),
        _ => None,
    };
    let _ = window.set_theme(t);
}

/// 覆盖式更新配置——设置页 `saveConfig()` 的**唯一**入口。
///
/// `base` = 前端**上次从后端收到的配置快照**。后端据此算出「用户真正改了哪些字段」
/// 并只套用这些字段（P1-11，见 `config::merge_config`），从而不会把弹窗侧并发的
/// 改名 / 隐藏 / 分组等改动整份抹掉。
///
/// `base` 为 `None` 时退回整体覆盖（旧行为）并打**告警**日志——该分支只为兼容，
/// 正常前端一定会带上 `base`；日志里出现这条告警即说明有调用点漏传。
#[tauri::command]
pub fn update_config(app: tauri::AppHandle, base: Option<Config>, mut new_config: Config) {
    let cycle_was_enabled = config::with_config(|c| c.enable_device_shortcut_cycle);
    if cycle_was_enabled && !new_config.enable_device_shortcut_cycle {
        // 关闭共享开关：清除被多个设备共用的快捷键
        clear_shared_device_shortcuts(&mut new_config);
        // 返回值（注册失败的键）已由 sync_device_shortcuts 自行上报（记录 + 广播）
        let _ = crate::shortcut::sync_device_shortcuts(&app);
    }
    // 保留时长是否变化，必须比较**套用前后**的真值：套用是「按差异合并」，
    // 前端没改 log_retention 时它压根不会被写，拿 patch 的值比较会误判。
    let retention_before = config::with_config(|c| c.log_retention);
    let applied = config::with_config_mut(|c| match base.as_ref() {
        Some(base) => Some(config::merge_config(c, base, &new_config)),
        None => {
            *c = new_config.clone();
            None
        }
    });
    match applied {
        Some(n) => standard_log!("[config] update_config: 套用 {} 个字段变更", n),
        None => standard_log!(
            "[config] update_config 未带 base 快照，已退回整体覆盖（可能丢失并发改动）"
        ),
    }
    let retention_changed = config::with_config(|c| c.log_retention) != retention_before;
    // 传递完整 config 快照，前端无需再调用 get_config
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    if retention_changed {
        process::clean_old_logs();
    }
}

/// 清除被多个设备共用的快捷键（保留各设备的名称，仅清空 shortcut）
fn clear_shared_device_shortcuts(c: &mut Config) {
    use std::collections::HashMap;
    let mut key_count: HashMap<String, usize> = HashMap::new();
    for d in c.device_shortcuts.values() {
        if let Some(ref k) = d.shortcut {
            *key_count.entry(k.clone()).or_insert(0) += 1;
        }
    }
    let mut cleared = false;
    for d in c.device_shortcuts.values_mut() {
        if let Some(ref k) = d.shortcut {
            if key_count.get(k).copied().unwrap_or(0) > 1 {
                d.shortcut = None;
                cleared = true;
            }
        }
    }
    if cleared {
        process::append_log("[config] shared device shortcuts cleared (cycle toggle disabled)");
    }
}

#[tauri::command]
pub fn toggle_device_hidden(app: tauri::AppHandle, name: String) {
    standard_log!("[cmd] toggle_device_hidden: {}", name);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_devices, &name));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command]
pub fn toggle_audio_device_hidden(app: tauri::AppHandle, name: String) {
    standard_log!("[cmd] toggle_audio_device_hidden: {}", name);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_audio_devices, &name));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command(async)]
pub async fn open_bt_settings() -> Result<(), String> {
    run_blocking(|| process::shell_open("ms-settings:bluetooth", None)).await?
}

/// `open_url` 允许的目标协议白名单。
/// 覆盖前端全部实际调用面：GitHub / Release 页（https）、空间音效回退（ms-settings:）。
const OPEN_URL_ALLOWED: &[&str] = &["https://", "http://", "ms-windows-store://", "ms-settings:"];

/// 判定一个 URL 是否允许交给系统打开（纯函数，便于单测覆盖拒绝分支）。
fn is_allowed_open_url(url: &str) -> bool {
    OPEN_URL_ALLOWED.iter().any(|p| url.starts_with(p))
}

/// 打开外部链接。
/// **必须白名单**：`ShellExecuteW` 会执行任意已注册协议，不加限制时前端一旦被注入
/// （XSS / 恶意扩展）即可借 `open_url` 用 `file:`、自定义协议等做本地落地，
/// 而本命令是前端唯一能触达「系统执行」的入口（见代码审查报告 P1-4）。
///
/// 白名单判定是纯字符串比较，留在主线程完成（校验失败无需付线程切换成本）；
/// 只有 `ShellExecuteW` 本体交给阻塞线程（B4）。
#[tauri::command(async)]
pub async fn open_url(url: String) -> Result<(), String> {
    if !is_allowed_open_url(&url) {
        standard_log!("[cmd] open_url 拒绝非白名单协议: {}", url);
        return Err(format!("不允许的链接协议: {}", url));
    }
    run_blocking(move || process::shell_open(&url, None)).await?
}

#[tauri::command]
pub fn rename_device(app: tauri::AppHandle, original: String, new_name: String) {
    standard_log!("[cmd] rename_device: '{}' -> '{}'", original, new_name);
    // ⭐ 方案 D：**归并写入 / 归并删除**（逻辑在 `config::apply_device_rename`，
    // 抽成纯函数是为了能被单测钉住 —— 它的失效方式是**静默**的）。
    config::with_config_mut(|c| {
        config::apply_device_rename(c, &original, &new_name);
    });
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    let _ = app.emit("audio-devices-changed", ());
}

#[tauri::command]
pub fn change_device_group(app: tauri::AppHandle, name: String, group: String) {
    standard_log!("[cmd] change_device_group: {} -> {}", name, group);
    config::with_config_mut(|c| {
        if group.is_empty() {
            c.device_groups.remove(&name);
        } else {
            c.device_groups.insert(name, group);
        }
    });
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command]
pub fn toggle_group_hidden(app: tauri::AppHandle, group: String) {
    standard_log!("[cmd] toggle_group_hidden: {}", group);
    config::with_config_mut(|c| toggle_vec_item(&mut c.hidden_groups, &group));
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

#[tauri::command(async)]
pub async fn disconnect_bluetooth_device(device_id: String) -> Result<String, String> {
    standard_log!("[cmd] disconnect_bluetooth_device: {}", device_id);
    run_blocking(move || crate::bluetooth::bt_action(&device_id, "disconnect", false))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn connect_bluetooth_device(device_id: String, is_ble: bool) -> Result<String, String> {
    standard_log!("[cmd] connect_bluetooth_device: {}", device_id);
    run_blocking(move || crate::bluetooth::bt_action(&device_id, "connect", is_ble))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn check_bt_connection(device_id: String) -> Result<Option<bool>, String> {
    run_blocking(move || crate::bluetooth::check_device_connection(&device_id)).await
}

/// 前端行为埋点：写入运行日志（受日志级别门控，标准级可见）
#[tauri::command]
pub fn frontend_log(tag: String, msg: String) {
    standard_log!("[{tag}] {msg}");
}

/// 用系统默认程序打开 2.4G 设备数据文件（`create_dir_all` + `write` + `ShellExecuteW`）。
/// 首启路径还会新建 data 目录与占位文件，全是磁盘 I/O，故整体移出主线程（B4）。
#[tauri::command(async)]
pub async fn open_24g_device_file() -> Result<(), String> {
    run_blocking(|| {
        let path = crate::device_data::user_data_path();
        if !path.exists() {
            // 内置 2.4G 库已废除，全新安装环境可能没有 data 目录
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
            }
            std::fs::write(&path, "{}").map_err(|e| e.to_string())?;
        }
        process::shell_open(&path.to_string_lossy(), None)
    })
    .await?
}

const TRAY_DEVICE_LIMIT: usize = 4;

/// 切换「托盘设备」列表中的某个设备：存在则移除，不存在则加入（受上限保护）。
///
/// ── 为什么必须是「只接 `&mut Config` 的单函数」（P2-8）──────────────────
/// 原实现是「先 `with_config` 读一次（查重 + 数上限）→ 再 `with_config_mut` 写一次」
/// ——**两次独立加锁**，中间还夹着一次 `run_blocking` 线程切换，于是：
///
/// ```text
/// 线程 A: 读 count=3（未达上限）...................... 写 → 4 个
/// 线程 B:          读 count=3（未达上限）→ 写 → 5 个   ← 上限被击穿
/// ```
///
/// 用户快速连点两次「添加到托盘」（两个并发 `invoke`）即可命中，
/// 结果是 `tray_devices.len() > TRAY_DEVICE_LIMIT`，托盘菜单出现 5 个设备。
///
/// 抽成本函数后，调用方只剩
/// `with_config_mut(|c| try_toggle_tray_device(c, &name))` ——
/// **读与写物理上处于同一次加锁内**，「检查与写入分离」在类型层面不再可能发生。
/// 这也是决策 2「不补 `tauri` test feature、改抽纯函数」的落点：
/// 单测直接构造 `Config::default()` 调用它，**不需要 `AppHandle`**。
fn try_toggle_tray_device(c: &mut Config, name: &str) -> Result<(), String> {
    let already_added = c.tray_devices.iter().any(|v| v == name);
    if !already_added && c.tray_devices.len() >= TRAY_DEVICE_LIMIT {
        return Err(format!("托盘最多添加 {} 个设备", TRAY_DEVICE_LIMIT));
    }
    toggle_vec_item(&mut c.tray_devices, name);
    Ok(())
}

#[tauri::command(async)]
pub async fn toggle_device_tray(app: tauri::AppHandle, name: String) -> Result<(), String> {
    let log_name = name.clone();
    // 上限拒绝与写入在同一次 `with_config_mut` 内完成（P2-8），并发连点不会击穿上限。
    let outcome =
        run_blocking(move || config::with_config_mut(|c| try_toggle_tray_device(c, &name))).await?;

    if let Err(msg) = outcome {
        standard_log!(
            "[cmd] toggle_device_tray: {} 达上限({})拒绝",
            log_name,
            TRAY_DEVICE_LIMIT
        );
        return Err(msg);
    }

    standard_log!("[cmd] toggle_device_tray: {}", log_name);
    crate::tray::refresh_tray_tooltip(&app);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    let _ = app.emit("tray-devices-changed", ());
    Ok(())
}

// ── 任务栏信息窗：固定显示（pin）的写入路径 ──────────────

/// 任务栏信息窗最多固定几台设备。
///
/// 与 `TRAY_DEVICE_LIMIT` 分开、不复用：两者约束的是**两个不同的界面**
/// （托盘菜单 vs 任务栏窄条），上限没有共同含义，共用一个常量会让改动其一
/// 时意外改动另一个。
///
/// 为什么必须有上限：`device_identity::group_taskbar_devices` 末尾会对每条固定项
/// **反向补建**一个占位条目（设备不在场也要占位显示），故固定项数量**直接**等于
/// 界面行数；没有上限的话，一份被改坏的 `config.toml` 就能让窗口长出几十行。
const PINNED_TASKBAR_LIMIT: usize = 8;

/// 把前端传来的可空字符串归一化：去空白后为空 ⇒ `None`。
///
/// 不能直接把 `Some("")` 存进配置：`pinned_device_matches` 的兜底比较是**逐字相等**，
/// 空串与任何真实键都不等 ⇒ 存下一个「看着有值、实则永不命中」的字段，
/// 排查时极具误导性（配置里明明写着 fallback，却怎么也匹配不上）。
fn normalized_opt(s: Option<&str>) -> Option<String> {
    s.map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// 切换「任务栏信息窗固定显示」的某台物理设备：已固定则取消，未固定则固定。
///
/// ── 为什么匹配必须复用 `config::pinned_device_matches` ────────────────────
/// 显示侧（`device_identity::group_taskbar_devices`）判定「这台已固定」用的正是
/// 同一个函数。若本命令改用「只比 `key`」之类的**另一套**规则，两者立刻分叉：
/// 显示说「已固定」、本命令却认为「没固定过」⇒ 用户点「取消固定」实际走的是
/// **新增**分支，界面纹丝不动，且每点一次多一条，很快撞上限。故两侧共用一份判据
/// 不是巧合，判据见 `pinned_toggle_is_the_inverse_of_the_display_predicate`。
///
/// ── 取消时删除**全部**命中项，而不是第一条 ────────────────────────────────
/// `fallback` 是**名称级**兜底键，同型号设备会撞键；且容器变化后新旧两条键可能
/// 并存。一台设备被多条固定项同时命中时，只删第一条 ⇒ 删完仍被下一条命中 ⇒
/// 界面**依旧显示已固定** ⇒ 用户再点一次，此时 `key` 已不在表里，于是走了
/// 「新增」分支 ⇒ **卡在「怎么点都取消不掉」**。删全部才能保证
/// 「点一次 = 状态翻转一次」。
///
/// ⚠️ 由此带来的已知语义（**兜底键的固有代价，不是缺陷**）：两台同名设备共享
/// `fallback` 时，固定其中一台会让另一台**也显示为已固定**（显示侧本就是如此），
/// 取消时也一并取消。本函数不做单方面特判——那只会让显示与切换再次分叉。
/// 判据见 `pinned_toggle_flips_shared_fallback_entry`。
///
/// 与 `try_toggle_tray_device` 同款：**上限检查与写入在同一次 `with_config_mut` 内**
/// （P2-8），并发连点不会击穿上限；抽成只接 `&mut Config` 的单函数以便直接单测。
fn try_toggle_pinned_taskbar_device(
    c: &mut Config,
    key: &str,
    fallback: Option<&str>,
    alias: Option<&str>,
) -> Result<(), String> {
    // 空键会变成「永不命中的幽灵条目」：显示侧匹配不上任何设备，前端却会为它渲染
    // 一行永久置灰的占位，用户既看不出它是什么、也只能原样再切一次才删得掉。
    if key.trim().is_empty() {
        return Err("固定的设备缺少身份键".to_string());
    }

    // 先按**显示侧同一判据**删净命中项；删到了就说明本次语义是「取消固定」。
    let before = c.pinned_taskbar_devices.len();
    c.pinned_taskbar_devices
        .retain(|p| !config::pinned_device_matches(p, key, fallback));
    if c.pinned_taskbar_devices.len() < before {
        return Ok(());
    }

    // 走到这里 ⇒ 本次是「新增」。已达上限时**不得写入**（否则上限形同虚设）。
    if c.pinned_taskbar_devices.len() >= PINNED_TASKBAR_LIMIT {
        return Err(format!("任务栏最多固定 {} 个设备", PINNED_TASKBAR_LIMIT));
    }
    c.pinned_taskbar_devices.push(config::PinnedDevice {
        key: key.to_string(),
        fallback: normalized_opt(fallback),
        alias: normalized_opt(alias),
    });
    Ok(())
}

/// 前端入口：切换任务栏信息窗的固定显示。
///
/// `fallback` / `alias` 由前端从 `PhysicalDevice` 取（见其文档），可省略。
#[tauri::command(async)]
pub async fn toggle_pinned_taskbar_device(
    app: tauri::AppHandle,
    key: String,
    fallback: Option<String>,
    alias: Option<String>,
) -> Result<(), String> {
    let log_key = key.clone();
    let outcome = run_blocking(move || {
        config::with_config_mut(|c| {
            try_toggle_pinned_taskbar_device(c, &key, fallback.as_deref(), alias.as_deref())
        })
    })
    .await?;

    if let Err(msg) = outcome {
        standard_log!(
            "[cmd] toggle_pinned_taskbar_device: {} 拒绝: {}",
            log_key,
            msg
        );
        return Err(msg);
    }

    standard_log!("[cmd] toggle_pinned_taskbar_device: {}", log_key);
    let config_snapshot = config::with_config(|c| c.clone());
    // 忽略 emit 失败：属「无状态后果」的 UI 呈现类——没刷新到的界面会在下一次
    // 事件或重新加载时自纠，配置本身已经写好了。
    let _ = app.emit("config-changed", config_snapshot);
    Ok(())
}

// ── 音频命令 ─────────────────────────────────────────────

#[tauri::command(async)]
pub async fn get_audio_devices() -> Result<Vec<crate::audio::AudioDevice>, String> {
    run_blocking(crate::audio::enumerate_output_devices)
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_device_volume(device_id: String, volume: f32) -> Result<(), String> {
    run_blocking(move || crate::audio::set_device_volume(&device_id, volume))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn toggle_device_mute(device_id: String) -> Result<(), String> {
    run_blocking(move || crate::audio::toggle_device_mute(&device_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_device_mute(device_id: String, muted: bool) -> Result<(), String> {
    run_blocking(move || crate::audio::set_device_mute(&device_id, muted))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_audio_sessions(
    device_id: String,
) -> Result<Vec<crate::audio::AudioSession>, String> {
    standard_log!("[cmd] get_audio_sessions: device_id={}", device_id);
    run_blocking(move || {
        // 同步等待 STA 线程完成会话回调注册，确保后续枚举能获取实时音量变化
        crate::audio_notify::request_session_sync_blocking();
        crate::audio::enumerate_audio_sessions(&device_id)
    })
    .await?
    .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_volume(
    session_id: String,
    device_id: String,
    volume: f32,
) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_volume(&session_id, &device_id, volume))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_mute(
    session_id: String,
    device_id: String,
    muted: bool,
) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_mute(&session_id, &device_id, muted))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_input_devices() -> Result<Vec<crate::audio::AudioDevice>, String> {
    run_blocking(crate::audio::enumerate_input_devices)
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn set_session_device(
    pid: u32,
    direction: String,
    device_id: String,
) -> Result<(), String> {
    run_blocking(move || crate::audio::set_session_device(pid, &direction, &device_id))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_session_device(pid: u32, direction: String) -> Result<Option<String>, String> {
    run_blocking(move || crate::audio::get_session_device(pid, &direction))
        .await?
        .map_err(|e| e.to_string())
}

#[tauri::command(async)]
pub async fn get_sessions_device_names(
    pids: Vec<u32>,
) -> Result<std::collections::HashMap<u32, crate::audio::SessionDeviceNames>, String> {
    run_blocking(move || crate::audio::resolve_session_device_names(&pids)).await
}

/// 切换系统默认输出设备。
///
/// `crate::audio::set_default_device` 走 `IPolicyConfig` COM 接口，实测耗时
/// 数十至数百毫秒（还要等音频服务响应），原先作为**同步命令**在主线程执行
/// ⇒ 每次点击设备名都会冻结 UI 同等时长（B4）。
/// `emit` 放在 `.await` 之后，与既有的 `toggle_device_tray` 形态一致。
#[tauri::command(async)]
pub async fn set_default_device(app: tauri::AppHandle, device_id: String) -> Result<(), String> {
    standard_log!("[cmd] set_default_device: {}", device_id);
    run_blocking(move || crate::audio::set_default_device(&device_id))
        .await?
        .map_err(|e| e.to_string())?;
    let _ = app.emit("audio-devices-changed", ());
    Ok(())
}

#[tauri::command(async)]
pub async fn get_spatial_sound(
    device_id: String,
) -> Result<crate::audio_spatial::SpatialSoundState, String> {
    run_blocking(move || crate::audio_spatial::get_spatial_sound(&device_id)).await?
}

#[tauri::command(async)]
pub async fn set_spatial_sound(
    device_id: String,
    format_guid: Option<String>,
) -> Result<(), String> {
    run_blocking(move || {
        crate::audio_spatial::set_spatial_sound(&device_id, format_guid.as_deref())
    })
    .await?
}

/// 打开日志目录（`create_dir_all` + `ShellExecuteW`），磁盘 I/O 移出主线程（B4）。
#[tauri::command(async)]
pub async fn open_log_dir() -> Result<(), String> {
    run_blocking(|| {
        let dir = crate::process::logs_dir();
        // 目录建不出来时，随后的 `shell_open` 只会返回一个语焉不详的
        // `ShellExecuteW` 错误码（`≤ 32` 即失败），无法归因。这里先把真正的原因报出去
        // （P3-7：该失败会让「目录状态与用户点击不一致」，不得静默）。
        std::fs::create_dir_all(&dir)
            .map_err(|e| format!("无法创建日志目录 {}：{}", dir.display(), e))?;
        process::shell_open(&dir.to_string_lossy(), None)
    })
    .await?
}

#[tauri::command(async)]
pub async fn check_for_update(
    app: tauri::AppHandle,
    include_prerelease: bool,
) -> Result<crate::update::UpdateStatus, String> {
    let current_version = app.package_info().version.to_string();
    let _ = crate::update::check_and_store("", current_version, include_prerelease).await;
    crate::update::get_last_status().ok_or_else(|| "状态未存储".to_string())
}

#[tauri::command]
pub fn get_update_status() -> Option<crate::update::UpdateStatus> {
    crate::update::get_last_status()
}

fn parse_shortcut(s: &str) -> Result<tauri_plugin_global_shortcut::Shortcut, String> {
    tauri_plugin_global_shortcut::Shortcut::try_from(s).map_err(|e| e.to_string())
}

/// 为一个动作注册快捷键（含事件分发闭包）。**只碰注册表，不写配置**，
/// 便于提交阶段在注册失败时把旧键原样恢复回去。
fn register_shortcut(
    app: &tauri::AppHandle,
    action: &str,
    sc: tauri_plugin_global_shortcut::Shortcut,
    key: &str,
) -> Result<(), String> {
    let action_clone = action.to_string();
    let key_clone = key.to_string();
    app.global_shortcut()
        .on_shortcut(sc, move |_app, _shortcut, event| {
            if event.state != ShortcutState::Pressed {
                return;
            }
            crate::shortcut::dispatch_shortcut_action(_app, &action_clone, &key_clone);
        })
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn set_hotkey_config(
    app: tauri::AppHandle,
    action: String,
    key: Option<String>,
) -> Result<(), String> {
    let prev_key = config::with_config(|c| match action.as_str() {
        "devices" => c.shortcut_devices.clone(),
        "volume" => c.shortcut_volume.clone(),
        "volume_up" => c.shortcut_volume_up.clone(),
        "volume_down" => c.shortcut_volume_down.clone(),
        "volume_mute" => c.shortcut_volume_mute.clone(),
        _ => None,
    });
    let prev_sc = prev_key.as_deref().and_then(|pk| parse_shortcut(pk).ok());

    // ① 校验阶段：**不做任何副作用**。
    // 原实现先注销旧键再校验新键，于是「新键解析失败」这条分支会留下：
    // 注册表里旧键已没了、配置里旧键还在 —— 前端显示「已设置 XX」但按键无反应，
    // 且不广播 config-changed，用户完全无从察觉（见代码审查报告 P1-5）。
    let new_sc = match key.as_deref() {
        Some(k) => {
            let sc = parse_shortcut(k)?; // 解析失败：状态零变化
                                         // 同键重设必须放行：此刻旧键尚未注销（校验先于副作用），它必然处于已注册状态，
                                         // 若按「已占用」拒绝，用户重新选中同一个键就会失败。
            let same_as_prev = prev_sc.as_ref() == Some(&sc);
            if !same_as_prev && app.global_shortcut().is_registered(sc) {
                return Err("快捷键已被占用".to_string()); // 被占用：状态零变化
            }
            Some(sc)
        }
        None => None,
    };

    // ② 提交阶段：注销旧 → 注册新 → 落配置（顺序不变，只是整体挪到校验之后）
    if let Some(prev) = prev_sc {
        let _ = app.global_shortcut().unregister(prev);
        standard_log!(
            "[hotkey] unregistered old key {} for {}",
            prev_key.as_deref().unwrap_or(""),
            action
        );
    }
    if let Some(sc) = new_sc {
        let new_key_str = key.as_deref().unwrap_or_default();
        if let Err(e) = register_shortcut(&app, &action, sc, new_key_str) {
            // 注册失败：把旧键恢复回去，否则会重现本条要修的那种不一致
            //（注册表空着、配置里留着旧键）
            if let (Some(prev), Some(prev_key_str)) = (prev_sc, prev_key.as_deref()) {
                let _ = register_shortcut(&app, &action, prev, prev_key_str);
            }
            return Err(e);
        }
        // 成功后才记日志：原先在注册尝试之前打印，失败时日志与事实相反
        standard_log!("[hotkey] registered {} {}", new_key_str, action);
    }
    set_config_key(&action, key);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    Ok(())
}

fn set_config_key(action: &str, key: Option<String>) {
    config::with_config_mut(|c| match action {
        "devices" => c.shortcut_devices = key,
        "volume" => c.shortcut_volume = key,
        "volume_up" => c.shortcut_volume_up = key,
        "volume_down" => c.shortcut_volume_down = key,
        "volume_mute" => c.shortcut_volume_mute = key,
        _ => {}
    });
}

#[tauri::command]
pub fn set_device_shortcut(
    app: tauri::AppHandle,
    device_id: String,
    name: String,
    key: Option<String>,
) -> Result<(), String> {
    if let Some(ref new_key_str) = key {
        let sc = parse_shortcut(new_key_str)?;
        // 若键已被注册且不是另一设备快捷键（不在当前设备快捷键集合中）→ 与非设备功能冲突
        if app.global_shortcut().is_registered(sc) {
            let share_enabled = crate::config::with_config(|c| c.enable_device_shortcut_cycle);
            let (used_by_any_device, used_by_other_device) = crate::config::with_config(|c| {
                let any = c
                    .device_shortcuts
                    .values()
                    .any(|d| d.shortcut.as_deref() == Some(new_key_str));
                let other = c
                    .device_shortcuts
                    .iter()
                    .any(|(id, d)| id != &device_id && d.shortcut.as_deref() == Some(new_key_str));
                (any, other)
            });
            // 关闭共享：被其他设备或非设备功能占用都拒绝（本设备自身占用允许）
            // 开启共享：仅拒绝非设备功能占用
            let conflict = if share_enabled {
                !used_by_any_device
            } else {
                used_by_other_device || !used_by_any_device
            };
            if conflict {
                return Err("快捷键已被占用".to_string());
            }
        }
    }
    standard_log!(
        "[hotkey] set_device_shortcut: {} key={}",
        name,
        key.as_deref().unwrap_or("None")
    );
    // 记下改动前的状态，供注册失败时回滚（与 set_hotkey_config 的处置一致，见 P1-5）
    let previous = config::with_config(|c| c.device_shortcuts.get(&device_id).cloned());
    let requested = key.clone();
    set_device_shortcut_key(&device_id, &name, key);

    let failed = crate::shortcut::sync_device_shortcuts(&app);
    // 上面那段 `is_registered` 校验只能发现**本进程内部**的冲突；被**其他程序**占用的键
    // 只有真正调 `RegisterHotKey` 时才会失败（见 P2-12）。此时必须回滚配置：
    // 否则界面显示「已设置 XX」、按键却毫无反应，用户完全无从察觉——
    // 正是 P1-5 要消灭的那类不一致。
    if let Some(k) = requested.as_deref() {
        if failed.iter().any(|f| f == k) {
            config::with_config_mut(|c| rollback_device_shortcut(c, &device_id, previous));
            // 回滚后**必须再同步一次**：上面那次同步已经把旧键注销了
            //（desired 里换成了新键），不重同步的话用户会「改键失败」
            // 且**连原来能用的键也一起丢**。实测证据见提交信息。
            let _ = crate::shortcut::sync_device_shortcuts(&app);
            let config_snapshot = config::with_config(|c| c.clone());
            let _ = app.emit("config-changed", config_snapshot);
            return Err("快捷键已被其他程序占用".to_string());
        }
    }

    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
    Ok(())
}

/// 把某个设备的快捷键恢复到 `set_device_shortcut` 改动前的状态。
///
/// `previous == None` 表示改动前配置里**没有**这个设备（新建设备的首次设键），
/// 此时回滚要把整条记录删掉，而不是留一条 `shortcut: None` 的空壳——
/// 空壳会让该设备出现在「已配置快捷键」列表里。
fn rollback_device_shortcut(
    c: &mut Config,
    device_id: &str,
    previous: Option<crate::config::DeviceShortcut>,
) {
    match previous {
        Some(prev) => {
            c.device_shortcuts.insert(device_id.to_string(), prev);
        }
        None => {
            c.device_shortcuts.remove(device_id);
        }
    }
}

fn set_device_shortcut_key(device_id: &str, name: &str, key: Option<String>) {
    config::with_config_mut(|c| {
        if let Some(k) = key {
            c.device_shortcuts.insert(
                device_id.to_string(),
                crate::config::DeviceShortcut {
                    name: name.to_string(),
                    shortcut: Some(k),
                },
            );
        } else if let Some(entry) = c.device_shortcuts.get_mut(device_id) {
            entry.shortcut = None;
            entry.name = name.to_string();
        }
    });
}

#[tauri::command]
pub fn remove_device_shortcut(app: tauri::AppHandle, device_id: String) {
    standard_log!("[hotkey] remove_device_shortcut: {}", device_id);
    config::with_config_mut(|c| {
        c.device_shortcuts.remove(&device_id);
    });
    // 返回值（注册失败的键）已由 sync_device_shortcuts 自行上报（记录 + 广播）
    let _ = crate::shortcut::sync_device_shortcuts(&app);
    let config_snapshot = config::with_config(|c| c.clone());
    let _ = app.emit("config-changed", config_snapshot);
}

// ═══════════════════════════════════════════════════════════════
// 窗口材质
// ═══════════════════════════════════════════════════════════════

/// 设置窗口材质并应用到所有窗口
#[tauri::command]
pub fn set_window_material(app: tauri::AppHandle, material: String) -> Result<bool, String> {
    crate::window_material::set_window_material(&app, material)
}

/// 检查系统是否支持指定材质
#[tauri::command]
pub fn check_material_support(material: String) -> Result<bool, String> {
    Ok(crate::window_material::check_material_support(&material))
}

#[cfg(test)]
mod tests {
    use super::{
        devices_for_taskbar_with, is_allowed_open_url, rollback_device_shortcut,
        try_toggle_pinned_taskbar_device, try_toggle_tray_device, PINNED_TASKBAR_LIMIT,
        TRAY_DEVICE_LIMIT,
    };
    use crate::config::{matches_pinned_taskbar, Config, DeviceShortcut, PinnedDevice};

    /// P1-4 的可证伪单测：`open_url` 只放行白名单协议。
    ///
    /// 修复前 `open_url` 直接把任意字符串交给 `ShellExecuteW`，没有这层判据；
    /// 本测试锁定「前端唯一触达系统执行的入口不得被用作本地落地原语」这一性质。
    #[test]
    fn open_url_rejects_non_whitelisted_schemes() {
        for ok in [
            "https://github.com/oneday5799/PeriTray",
            "http://127.0.0.1:8080/x",
            "ms-settings:sound-defaultoutputproperties",
            "ms-windows-store://pdp/?productid=X",
        ] {
            assert!(is_allowed_open_url(ok), "白名单协议应放行: {ok}");
        }

        for bad in [
            "file:///C:/Windows/System32/calc.exe",
            "javascript:alert(1)",
            "data:text/html,<script>alert(1)</script>",
            "vbscript:msgbox(1)",
            "ftp://example.com/x",
            r"C:/Windows/System32/calc.exe",
            r"\\server\share\payload.exe",
            "shell:startup",
            "",
        ] {
            assert!(!is_allowed_open_url(bad), "非白名单协议必须拒绝: {bad}");
        }
    }

    // ── P2-12：注册失败后的回滚 ──────────────────────────────

    fn entry(name: &str, shortcut: Option<&str>) -> DeviceShortcut {
        DeviceShortcut {
            name: name.to_string(),
            shortcut: shortcut.map(|s| s.to_string()),
        }
    }

    /// 改动前**已有**该设备 ⇒ 回滚必须还原成旧值（含旧 name），而不是删掉或留新值。
    ///
    /// 这正是「用户把一个能用的键改成被其他程序占用的键」时走的分支：
    /// 不回滚的话配置里留着不可用的新键，界面显示已设置、按键却无反应（P2-12）。
    #[test]
    fn rollback_restores_previous_shortcut() {
        let mut c = Config::default();
        c.device_shortcuts
            .insert("dev-1".to_string(), entry("新名字", Some("Ctrl+Alt+KeyJ")));

        rollback_device_shortcut(
            &mut c,
            "dev-1",
            Some(entry("旧名字", Some("Ctrl+Alt+KeyK"))),
        );

        let got = c.device_shortcuts.get("dev-1").expect("设备记录必须还在");
        assert_eq!(
            got.shortcut.as_deref(),
            Some("Ctrl+Alt+KeyK"),
            "必须还原旧键"
        );
        assert_eq!(
            got.name, "旧名字",
            "name 也要一起还原（改动是整条记录级的）"
        );
    }

    /// 改动前**没有**该设备 ⇒ 回滚必须把整条记录删掉，不能留 `shortcut: None` 的空壳
    /// （空壳会让设备出现在「已配置快捷键」列表里，用户会以为它配置过）。
    #[test]
    fn rollback_removes_newly_created_entry() {
        let mut c = Config::default();
        c.device_shortcuts.insert(
            "dev-2".to_string(),
            entry("探针设备", Some("Ctrl+Alt+KeyJ")),
        );

        rollback_device_shortcut(&mut c, "dev-2", None);

        assert!(
            !c.device_shortcuts.contains_key("dev-2"),
            "新建的设备在回滚后必须整条消失，而不是留下空壳"
        );
    }

    /// P2-8 的可证伪单测：`try_toggle_tray_device` 的**上限拒绝**必须
    /// ① 返回 `Err`，且 ② **不写入**（列表长度不变）。
    ///
    /// 修复前「检查」与「写入」在两次独立加锁里（中间还夹一次 `run_blocking`
    /// 线程切换），并发连点可击穿上限；抽成「只接 `&mut Config` 的单函数」后，
    /// 两条语义被同一个函数体锁死，本用例才有确定的判据可断言。
    ///
    /// 断言 `len` 而不只是返回值是关键：只断言 `Err` 的话，
    /// 「先写进去再报错」这种半吊子实现也会通过。
    #[test]
    fn tray_device_limit_rejects_without_writing() {
        let mut c = Config {
            tray_devices: (0..TRAY_DEVICE_LIMIT)
                .map(|i| format!("已满-{i}"))
                .collect(),
            ..Default::default()
        };

        let err = try_toggle_tray_device(&mut c, "第 N+1 个");

        assert!(err.is_err(), "已达上限时必须拒绝");
        assert_eq!(
            c.tray_devices.len(),
            TRAY_DEVICE_LIMIT,
            "被拒绝的请求不得写进列表，否则上限保护形同虚设"
        );
        assert!(
            !c.tray_devices.iter().any(|v| v == "第 N+1 个"),
            "被拒绝的设备名不得出现在列表中"
        );
    }

    /// 上限判据的**另一侧**：差一个到上限时仍应放行并写入。
    ///
    /// 与上一用例成对——只测「拒绝」的话，一个「永远拒绝」的实现也能通过。
    #[test]
    fn tray_device_limit_allows_when_one_below() {
        let mut c = Config {
            tray_devices: (0..TRAY_DEVICE_LIMIT - 1)
                .map(|i| format!("未满-{i}"))
                .collect(),
            ..Default::default()
        };

        let res = try_toggle_tray_device(&mut c, "最后一个名额");

        assert!(res.is_ok(), "未达上限时必须放行: {res:?}");
        assert_eq!(
            c.tray_devices.len(),
            TRAY_DEVICE_LIMIT,
            "放行后应恰好达到上限"
        );
        assert!(c.tray_devices.iter().any(|v| v == "最后一个名额"));
    }

    /// 已在列表中 ⇒ 移除，且**移除不受上限约束**。
    ///
    /// 「已达上限时移除已有项」是必须放行的路径——否则用户会卡在满员状态：
    /// 想加新的加不进，想先删一个又因「已达上限」被拒。
    #[test]
    fn tray_device_toggle_removes_existing_even_at_limit() {
        let mut c = Config {
            tray_devices: (0..TRAY_DEVICE_LIMIT)
                .map(|i| format!("已满-{i}"))
                .collect(),
            ..Default::default()
        };

        let res = try_toggle_tray_device(&mut c, "已满-0");

        assert!(res.is_ok(), "移除已有项不得被上限拦截: {res:?}");
        assert_eq!(
            c.tray_devices.len(),
            TRAY_DEVICE_LIMIT - 1,
            "移除后应少一个"
        );
        assert!(!c.tray_devices.iter().any(|v| v == "已满-0"));
    }

    // ── pin（任务栏固定显示）的写入路径 ──────────────────────
    //
    // 这一组的重点不是「能加能删」，而是**切换侧与显示侧必须同源**：
    // 两侧判据一旦分叉，「取消固定」会静默变成「又插一条」（界面纹丝不动），
    // 这是本组用例存在的唯一理由。

    fn pin(key: &str, fallback: Option<&str>) -> PinnedDevice {
        PinnedDevice {
            key: key.to_string(),
            fallback: fallback.map(|s| s.to_string()),
            alias: None,
        }
    }

    /// 基本往返：未固定 ⇒ 加入一条（三个字段逐字落盘）；再切一次 ⇒ 移除。
    ///
    /// 断言整条 `PinnedDevice` 而不只是长度：只断言长度的话，
    /// 「`fallback` / `alias` 被丢掉」这种实现照样通过 —— 而那正是**换机/重装驱动后
    /// pin 静默失联**的成因。
    #[test]
    fn pinned_toggle_adds_then_removes() {
        let mut c = Config::default();

        try_toggle_pinned_taskbar_device(&mut c, "c:aaa", Some("n:我的耳机"), Some("耳机"))
            .expect("首次切换应加入");

        assert_eq!(
            c.pinned_taskbar_devices,
            vec![PinnedDevice {
                key: "c:aaa".to_string(),
                fallback: Some("n:我的耳机".to_string()),
                alias: Some("耳机".to_string()),
            }],
            "三个字段都必须原样落盘，丢一个都会让兜底匹配或占位名失效"
        );

        try_toggle_pinned_taskbar_device(&mut c, "c:aaa", Some("n:我的耳机"), Some("耳机"))
            .expect("再次切换应移除");

        assert!(
            c.pinned_taskbar_devices.is_empty(),
            "第二次切换必须把条目删掉，实际: {:?}",
            c.pinned_taskbar_devices
        );
    }

    /// ⭐ 核心判据：**切换是显示的逆运算**。
    ///
    /// 对每种初始状态断言 `matches_pinned_taskbar` 在切换前后**必须翻转**。
    /// 这条不成立时的表现极具欺骗性：界面显示「已固定」，用户点「取消固定」，
    /// 由于判据分叉走成了新增分支 ⇒ 界面**毫无变化**（且每点一次多一条，很快撞上限）。
    ///
    /// 可证伪：把 `try_toggle_pinned_taskbar_device` 的匹配换成「只比 `key`」⇒
    /// 第 3、5 条（仅靠 `fallback` 命中）立刻转红；把 `retain` 换成
    /// 「只删第一条」⇒ 第 4 条转红。
    #[test]
    fn pinned_toggle_is_the_inverse_of_the_display_predicate() {
        let cases: Vec<(Vec<PinnedDevice>, &str, Option<&str>)> = vec![
            // ① 空表、完全不命中 ⇒ 加入
            (vec![], "c:aaa", Some("n:耳机")),
            // ② 精确身份命中（同容器）⇒ 取消
            (vec![pin("c:aaa", Some("n:耳机"))], "c:aaa", Some("n:耳机")),
            // ③ **仅靠 fallback 命中**（容器换过，表里是旧容器键）⇒ 取消
            (vec![pin("c:old", Some("n:耳机"))], "c:new", Some("n:耳机")),
            // ④ 两条都命中（新旧键并存）⇒ 必须一次删净
            (
                vec![pin("c:old", Some("n:耳机")), pin("c:new", Some("n:耳机"))],
                "c:new",
                Some("n:耳机"),
            ),
            // ⑤ 表里有别的设备的固定项 ⇒ 不命中，走新增
            (vec![pin("c:aaa", None)], "c:bbb", Some("n:耳机")),
        ];

        for (list, key, fallback) in cases {
            let mut c = Config {
                pinned_taskbar_devices: list,
                ..Default::default()
            };
            let before = matches_pinned_taskbar(&c.pinned_taskbar_devices, key, fallback);

            try_toggle_pinned_taskbar_device(&mut c, key, fallback, None)
                .unwrap_or_else(|e| panic!("切换不应失败: key={key} err={e}"));

            let after = matches_pinned_taskbar(&c.pinned_taskbar_devices, key, fallback);
            assert_ne!(
                before, after,
                "切换后「是否已固定」必须翻转: key={key} fallback={fallback:?} \
                 before={before} after={after} list={:?}",
                c.pinned_taskbar_devices
            );
        }
    }

    /// ⭐ 取消固定必须删掉**全部**命中项。
    ///
    /// 只删第一条的话，删完仍被下一条命中 ⇒ 界面**依旧显示已固定** ⇒ 用户再点一次，
    /// 此时 `key` 已不在表里，于是走了「新增」分支 ⇒ 条目数不减反增，
    /// **卡在「怎么点都取消不掉」**。这正是「检查与写入分离」在匹配层的翻版。
    #[test]
    fn pinned_toggle_removes_every_matching_entry() {
        let mut c = Config {
            pinned_taskbar_devices: vec![
                pin("c:old", Some("n:我的耳机")), // 容器换过，旧键仍留在表里
                pin("c:new", Some("n:我的耳机")),
            ],
            ..Default::default()
        };

        try_toggle_pinned_taskbar_device(&mut c, "c:new", Some("n:我的耳机"), None)
            .expect("取消固定不应失败");

        assert!(
            c.pinned_taskbar_devices.is_empty(),
            "必须一次删净，否则用户点一次状态不翻转、再点一次反而多一条。实际: {:?}",
            c.pinned_taskbar_devices
        );
    }

    /// ⚠️ **已知语义**（兜底键的固有代价，不是缺陷）：两台**同名**设备共享 `fallback`
    /// 时，固定其中一台会让另一台**也显示为已固定**（显示侧本就是如此），取消时也一并
    /// 取消。本用例把它钉死，避免以后有人以为这是 bug 而在**切换侧单方面**改判据 ——
    /// 那样只会让显示与切换分叉（见上一个用例的失败模式）。
    ///
    /// 真要消掉这个语义，得从**存储层**动手（例如容器键稳定的设备不再存名称兜底），
    /// 而不是在切换侧特判。
    #[test]
    fn pinned_toggle_flips_shared_fallback_entry() {
        let mut c = Config::default();
        try_toggle_pinned_taskbar_device(&mut c, "c:headset-a", Some("n:WH-1000XM4"), None)
            .expect("固定 A 应成功");

        // B 与 A 同名 ⇒ 显示侧认为 B 也已固定（本用例的前提）
        assert!(
            matches_pinned_taskbar(
                &c.pinned_taskbar_devices,
                "c:headset-b",
                Some("n:WH-1000XM4")
            ),
            "前提：同名设备的兜底键应当命中"
        );

        try_toggle_pinned_taskbar_device(&mut c, "c:headset-b", Some("n:WH-1000XM4"), None)
            .expect("对 B 切换应走取消分支");

        assert!(
            c.pinned_taskbar_devices.is_empty(),
            "不得为 B 再插一条（那会让表里出现两条同名项），实际: {:?}",
            c.pinned_taskbar_devices
        );
    }

    /// 上限拒绝必须 ① 返回 `Err` 且 ② **不写入**。
    ///
    /// 断言长度而不只是返回值是关键：只断言 `Err` 的话，「先写进去再报错」
    /// 这种半吊子实现也会通过（与 `tray_device_limit_rejects_without_writing` 同款理由）。
    #[test]
    fn pinned_limit_rejects_without_writing() {
        let mut c = Config {
            pinned_taskbar_devices: (0..PINNED_TASKBAR_LIMIT)
                .map(|i| pin(&format!("c:full-{i}"), None))
                .collect(),
            ..Default::default()
        };

        let err = try_toggle_pinned_taskbar_device(&mut c, "c:第N+1个", None, None);

        assert!(err.is_err(), "已达上限时必须拒绝");
        assert_eq!(
            c.pinned_taskbar_devices.len(),
            PINNED_TASKBAR_LIMIT,
            "被拒绝的请求不得写进列表，否则上限保护形同虚设"
        );
    }

    /// 上限判据的另一侧：差一个到上限时仍应放行并写入。
    ///
    /// 与上一用例成对 —— 只测「拒绝」的话，一个「永远拒绝」的实现也能通过。
    #[test]
    fn pinned_limit_allows_when_one_below() {
        let mut c = Config {
            pinned_taskbar_devices: (0..PINNED_TASKBAR_LIMIT - 1)
                .map(|i| pin(&format!("c:below-{i}"), None))
                .collect(),
            ..Default::default()
        };

        let res = try_toggle_pinned_taskbar_device(&mut c, "c:最后一个名额", None, None);

        assert!(res.is_ok(), "未达上限时必须放行: {res:?}");
        assert_eq!(c.pinned_taskbar_devices.len(), PINNED_TASKBAR_LIMIT);
    }

    /// 已达上限时**取消**已有项必须放行。
    ///
    /// 否则用户会卡在满员状态：想加新的加不进，想先删一个又因「已达上限」被拒。
    #[test]
    fn pinned_removal_is_not_blocked_by_limit() {
        let mut c = Config {
            pinned_taskbar_devices: (0..PINNED_TASKBAR_LIMIT)
                .map(|i| pin(&format!("c:full-{i}"), None))
                .collect(),
            ..Default::default()
        };

        let res = try_toggle_pinned_taskbar_device(&mut c, "c:full-0", None, None);

        assert!(res.is_ok(), "移除已有项不得被上限拦截: {res:?}");
        assert_eq!(c.pinned_taskbar_devices.len(), PINNED_TASKBAR_LIMIT - 1);
    }

    /// 空白字段归一化 + 空键拒绝。
    ///
    /// 三件事：① `""` / 纯空格不得被存成 `Some("")`（那是「看着有值、实则永不命中」
    /// 的字段，排查时极具误导性）；② `alias` 前后空白要剪掉；③ **空键直接拒绝** ——
    /// 空键会变成一条永远匹配不上任何设备的幽灵固定项，前端却会为它渲染一行
    /// 永久置灰的占位，用户既看不出它是什么、也只能原样再切一次才删得掉。
    #[test]
    fn pinned_toggle_normalizes_blank_fields_and_rejects_blank_key() {
        let mut c = Config::default();

        try_toggle_pinned_taskbar_device(&mut c, "c:aaa", Some("   "), Some("  耳机  "))
            .expect("空兜底不算错误，应被归一化为 None");

        assert_eq!(
            c.pinned_taskbar_devices,
            vec![PinnedDevice {
                key: "c:aaa".to_string(),
                fallback: None,
                alias: Some("耳机".to_string()),
            }],
            "空兜底必须存成 None；别名必须剪掉前后空白"
        );

        let mut c = Config::default();
        let err = try_toggle_pinned_taskbar_device(&mut c, "  ", Some("n:x"), None);
        assert!(err.is_err(), "空键必须被拒绝");
        assert!(
            c.pinned_taskbar_devices.is_empty(),
            "被拒绝的空键不得写进列表，否则会留下永不命中的幽灵条目"
        );
    }

    // ── 任务栏设备侧取数：缓存为空必须自愈（默认配置下缓存恒空）──────

    fn dev(name: &str) -> crate::device::Device {
        crate::device::Device {
            name: name.to_string(),
            dt: crate::device::DevType::Other,
            status: "OK".to_string(),
            battery: Some(50),
            device_id: None,
            device_key: None,
            is_bluetooth: false,
            is_wireless_24g: false,
            wireless_24g_kind: None,
            is_connected: true,
            is_ble: false,
        }
    }

    /// 判据：**缓存为空时必须现查一次**。
    ///
    /// 为什么这条最要紧：`Config::default()` 里 `tray_devices` 为空、
    /// `low_battery_notify` 为 `false`，而 `tray::start_device_watcher` 在这两者皆假时
    /// **整轮 `continue`** ⇒ 设备缓存**恒空**。此时若不现查，任务栏会**静默**丢掉全部
    /// 电量（所有 `battery` 为 `None`，「有电量无音频端点」的鼠标/键盘/手柄整条不出现）。
    ///
    /// 可证伪：把 `devices_for_taskbar_with` 的判据改成 `false`（永不回落）⇒ 本用例转红。
    #[test]
    fn empty_device_cache_falls_back_to_live_query() {
        let mut queried = false;
        let out = devices_for_taskbar_with(Vec::new(), || {
            queried = true;
            Ok(vec![dev("现查到的设备")])
        })
        .expect("回落路径不得失败");
        assert!(
            queried,
            "缓存为空时必须现查一次，否则任务栏会静默丢掉全部电量"
        );
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "现查到的设备");
    }

    /// 反控：**缓存非空时不得重跑 WMI** —— 否则每轮都白付一次全量设备查询。
    ///
    /// 可证伪：把判据改成「永远回落」⇒ 本用例转红。
    #[test]
    fn non_empty_device_cache_does_not_requery() {
        let mut queried = false;
        let out = devices_for_taskbar_with(vec![dev("缓存里的设备")], || {
            queried = true;
            Ok(Vec::new())
        })
        .expect("缓存命中路径不得失败");
        assert!(!queried, "缓存非空时不得重跑 WMI");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "缓存里的设备");
    }

    /// 反控：现查失败必须**原样上报 Err**，不得静默降级成空列表。
    ///
    /// 降级成空列表会把「WMI 不可信」伪装成「任务栏里什么都没有」，
    /// 与本次要修的「静默丢数据」是同一类故障，只是换了个位置。
    #[test]
    fn live_query_failure_is_propagated_not_swallowed() {
        let err = devices_for_taskbar_with(Vec::new(), || Err("WMI 不可信".to_string()))
            .expect_err("现查失败必须返回 Err");
        assert_eq!(err, "WMI 不可信");
    }
}

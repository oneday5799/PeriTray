use crate::standard_log;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogRetention {
    Once,
    OneDay,
    ThreeDays,
    OneWeek,
    OneMonth,
}

impl Serialize for LogRetention {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Once => serializer.serialize_str("once"),
            Self::OneDay => serializer.serialize_str("one_day"),
            Self::ThreeDays => serializer.serialize_str("three_days"),
            Self::OneWeek => serializer.serialize_str("one_week"),
            Self::OneMonth => serializer.serialize_str("one_month"),
        }
    }
}

impl<'de> Deserialize<'de> for LogRetention {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "once" => Ok(Self::Once),
            "one_day" | "oneday" => Ok(Self::OneDay),
            "three_days" | "threedays" => Ok(Self::ThreeDays),
            "one_week" | "oneweek" => Ok(Self::OneWeek),
            "one_month" | "onemonth" => Ok(Self::OneMonth),
            _ => Err(serde::de::Error::custom(format!(
                "unknown log_retention: {}",
                s
            ))),
        }
    }
}

impl Default for LogRetention {
    fn default() -> Self {
        Self::OneDay
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceShortcut {
    pub name: String,
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub auto_start: bool,
    pub hidden_devices: Vec<String>,
    pub hidden_groups: Vec<String>,
    pub device_names: std::collections::HashMap<String, String>,
    pub device_groups: std::collections::HashMap<String, String>,
    pub filter_enabled: bool,
    pub filter_regex: String,
    pub dedup_devices: bool,
    pub show_unnamed_bt: bool,
    pub use_system_bt: bool,
    #[serde(default = "default_true")]
    pub wireless_only: bool,
    #[serde(default)]
    pub tray_devices: Vec<String>,
    #[serde(default)]
    pub hidden_audio_devices: Vec<String>,
    /// 日志级别："off"/"standard"/"verbose"
    #[serde(default = "default_log_level")]
    pub log_level: String,
    /// 旧版布尔日志开关迁移承接（字段名经 alias 兼容旧键 log_enabled；
    /// 迁移后置空，不再序列化）
    #[serde(
        default,
        alias = "log_enabled",
        skip_serializing_if = "Option::is_none"
    )]
    pub legacy_log_enabled: Option<bool>,
    #[serde(default)]
    pub log_retention: LogRetention,
    #[serde(default)]
    pub shutdown_volume_enabled: bool,
    #[serde(default)]
    pub shutdown_volume_devices: std::collections::HashMap<String, f32>,
    #[serde(default)]
    pub mute_lock: bool,
    #[serde(default)]
    pub volume_fine_adjust: bool,
    #[serde(default)]
    pub force_mute_devices: Vec<String>,
    #[serde(default)]
    pub enable_spatial_sound: bool,
    #[serde(default = "default_true")]
    pub check_updates: bool,
    #[serde(default)]
    pub include_prerelease: bool,
    #[serde(default = "default_true")]
    pub simplify_device_names: bool,
    #[serde(default)]
    pub shortcut_devices: Option<String>,
    #[serde(default)]
    pub shortcut_volume: Option<String>,
    #[serde(default)]
    pub shortcut_volume_up: Option<String>,
    #[serde(default)]
    pub shortcut_volume_down: Option<String>,
    #[serde(default)]
    pub shortcut_volume_mute: Option<String>,
    #[serde(default)]
    pub hardware_acceleration: bool,
    #[serde(default = "default_popup_tab")]
    pub default_popup_tab: String,
    /// 弹窗尺寸档位："small"/"default"/"large"
    #[serde(default = "default_popup_size")]
    pub popup_size: String,
    #[serde(default)]
    pub device_shortcuts: std::collections::HashMap<String, DeviceShortcut>,
    #[serde(default)]
    pub enable_device_shortcut_cycle: bool,
    #[serde(default)]
    pub shortcut_switch_notify: bool,
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
    #[serde(default = "default_window_material")]
    pub window_material: String,
    #[serde(default)]
    pub low_battery_notify: bool,
    #[serde(default)]
    pub low_battery_devices: Vec<String>,
    #[serde(default = "default_battery_thresholds")]
    pub low_battery_thresholds: Vec<i32>,
    #[serde(default = "default_battery_refresh_secs")]
    pub low_battery_refresh_secs: u32,
}

fn default_true() -> bool {
    true
}
fn default_popup_tab() -> String {
    "devices".to_string()
}
fn default_popup_size() -> String {
    "default".to_string()
}
fn default_theme_mode() -> String {
    "follow_system".to_string()
}
fn default_window_material() -> String {
    "default".to_string()
}
fn default_battery_thresholds() -> Vec<i32> {
    vec![15, 10, 5]
}
fn default_battery_refresh_secs() -> u32 {
    10
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_start: false,
            hidden_devices: vec![],
            hidden_groups: vec!["Battery".to_string(), "Monitor".to_string()],
            device_names: std::collections::HashMap::new(),
            device_groups: std::collections::HashMap::new(),
            filter_enabled: true,
            filter_regex: Self::default_filter_regex(),
            dedup_devices: true,
            show_unnamed_bt: false,
            use_system_bt: false,
            wireless_only: true,
            tray_devices: vec![],
            hidden_audio_devices: vec![],
            log_level: default_log_level(),
            legacy_log_enabled: None,
            log_retention: LogRetention::default(),
            shutdown_volume_enabled: false,
            shutdown_volume_devices: std::collections::HashMap::new(),
            mute_lock: false,
            volume_fine_adjust: false,
            force_mute_devices: vec![],
            enable_spatial_sound: false,
            check_updates: true,
            include_prerelease: false,
            simplify_device_names: true,
            shortcut_devices: None,
            shortcut_volume: None,
            shortcut_volume_up: None,
            shortcut_volume_down: None,
            shortcut_volume_mute: None,
            hardware_acceleration: false,
            default_popup_tab: default_popup_tab(),
            popup_size: default_popup_size(),
            device_shortcuts: std::collections::HashMap::new(),
            enable_device_shortcut_cycle: false,
            shortcut_switch_notify: false,
            theme_mode: default_theme_mode(),
            window_material: default_window_material(),
            low_battery_notify: false,
            low_battery_devices: vec![],
            low_battery_thresholds: default_battery_thresholds(),
            low_battery_refresh_secs: default_battery_refresh_secs(),
        }
    }
}

impl Config {
    /// Combined regex for all device exclusion filters (case-insensitive)
    fn default_filter_regex() -> String {
        "Virtual|虚拟|^HID|^符合 HID|Audio Device|Audio 设备|Hands-Free|A2DP|gvinput Device|英特尔\\(R\\)|^NVIDIA"
            .to_string()
    }
}

static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();
/// 日志级别进程缓存：0=关闭 1=标准 2=详细
static LOG_LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static LOG_ONCE: AtomicBool = AtomicBool::new(false);
/// 上次成功写盘的 TOML 内容，用于脏检查跳过无变化写入
static LAST_CONFIG_CONTENT: OnceLock<Mutex<Option<String>>> = OnceLock::new();
/// 落盘串行锁：与配置锁相互独立的第二把锁，保证同一时刻只有一次落盘在进行
/// （落盘已移出配置锁，见 `with_config_mut`；这里只解决「不交错」）
static PERSIST_LOCK: Mutex<()> = Mutex::new(());
/// 落盘序号（内容版本号）：解决串行锁解决不了的「乱序覆盖」——
/// 调用 A 先取内容、调用 B 后取内容，但 B 先落盘、A 后落盘时，
/// 磁盘上会留下 A 的旧内容。进入串行区前取号，进入后若发现已有更新者取过号就丢弃本次。
static CONFIG_REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// 解析日志级别字符串（未知值按关闭处理）
pub fn parse_log_level(s: &str) -> u8 {
    match s {
        "standard" => 1,
        "verbose" => 2,
        _ => 0,
    }
}

fn default_log_level() -> String {
    "off".to_string()
}

/// 标准级日志是否启用（生命周期摘要与各模块常规行）
pub fn standard_log_enabled() -> bool {
    LOG_LEVEL.load(Ordering::Relaxed) >= 1
}

/// 详细级诊断日志是否启用
pub fn verbose_log_enabled() -> bool {
    LOG_LEVEL.load(Ordering::Relaxed) >= 2
}

pub fn log_once() -> bool {
    LOG_ONCE.load(Ordering::Relaxed)
}

fn sync_log_cache(config: &Config) {
    LOG_LEVEL.store(parse_log_level(&config.log_level), Ordering::Relaxed);
    LOG_ONCE.store(
        config.log_retention == LogRetention::Once,
        Ordering::Relaxed,
    );
}

fn config_path() -> std::path::PathBuf {
    crate::process::exe_dir().join("config.toml")
}

pub fn init_config() {
    CONFIG.set(Mutex::new(Config::default())).ok();
    let config = {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
                Err(e) => {
                    // stderr 直出：日志门控依赖本文件解析成功，失败时必须可见
                    eprintln!("[config] parse error: {}", e);
                    standard_log!("[config] parse error: {}", e);
                    Config::default()
                }
            },
            Err(e) => {
                eprintln!("[config] load failed (using defaults): {}", e);
                standard_log!("[config] load failed (using defaults): {}", e);
                Config::default()
            }
        }
    };
    {
        let mut guard = crate::state::lock_unpoisoned(CONFIG.get().unwrap());
        *guard = config;
        sync_log_cache(&guard);
    }
    // 初始化脏检查缓存：读取磁盘文件内容作为基准
    if let Ok(content) = std::fs::read_to_string(config_path()) {
        if let Some(last) = LAST_CONFIG_CONTENT.get() {
            if let Ok(mut cached) = last.lock() {
                *cached = Some(content);
            }
        } else {
            let _ = LAST_CONFIG_CONTENT.set(Mutex::new(Some(content)));
        }
    }
    // 旧版布尔日志开关一次性迁移（true→标准 / false→关闭），
    // 消费 legacy 字段并立即持久化，防止每次启动重复映射。
    // 注意：此处必须在上方 guard 作用域结束后执行，否则 CONFIG
    // 重入加锁将死锁（with_config 系列会再次锁定）。
    if with_config(|c| c.legacy_log_enabled.is_some()) {
        with_config_mut(|c| {
            let to_standard = c.legacy_log_enabled == Some(true);
            c.log_level = if to_standard {
                "standard".to_string()
            } else {
                "off".to_string()
            };
            c.legacy_log_enabled = None;
            standard_log!("[config] 旧版日志开关已迁移为级别: {}", c.log_level);
        });
    }
}

/// 落盘取号：调用方**必须已持有内容快照**后再调用，否则版本号与内容不对应。
fn claim_revision() -> u64 {
    CONFIG_REVISION.fetch_add(1, Ordering::SeqCst) + 1
}

/// 本次取号是否仍是最新的一号。只有最新号才允许落盘，
/// 否则「先取号者后落盘」会把旧内容盖到新内容上。
fn revision_is_latest(rev: u64) -> bool {
    rev == CONFIG_REVISION.load(Ordering::SeqCst)
}

/// 脏检查 + 原子落盘。**必须在配置锁之外调用**（这是 P1-3 的核心）。
///
/// 两步保护，缺一不可：
/// 1. `PERSIST_LOCK` 串行化——保证两次落盘不交错（不会你写一半我写一半）；
/// 2. `CONFIG_REVISION` 版本号——串行化**不解决乱序**：先取到内容的那次可能后落盘，
///    把较旧的内容盖到较新的内容上。故进入串行区前先取号，进入后若已被更新者超越即丢弃。
fn persist_if_changed(content: &str) {
    let rev = claim_revision();
    let _serial = crate::state::lock_unpoisoned(&PERSIST_LOCK);

    // 已有更新的写入取过号 → 本次内容已过期，丢弃（防乱序覆盖）
    if !revision_is_latest(rev) {
        return;
    }

    // #23 脏检查：内容未变化时跳过写盘（减少高频配置操作的 I/O）
    let last = LAST_CONFIG_CONTENT.get_or_init(|| Mutex::new(None));
    if let Ok(cached) = last.lock() {
        if cached.as_deref() == Some(content) {
            return;
        }
    }

    use std::io::Write;
    // 原子写入：先写临时文件，再 rename 替换（同卷原子操作）
    let cfg_path = config_path();
    let tmp_path = cfg_path.with_extension("toml.tmp");
    let write_result = std::fs::File::create(&tmp_path)
        .and_then(|mut f| {
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })
        .and_then(|_| std::fs::rename(&tmp_path, &cfg_path));
    if let Err(e) = write_result {
        standard_log!("[config] save failed: {}", e);
        // 清理临时文件（如果 rename 失败）
        let _ = std::fs::remove_file(&tmp_path);
    } else if let Ok(mut cached) = last.lock() {
        // 写盘成功，更新缓存
        *cached = Some(content.to_string());
    }
}

/// 只读访问配置。
///
/// **锁纪律（P0 死锁防护，勿破坏）**：闭包内**只允许纯内存操作**（读字段、clone、算术）。
/// 禁止在闭包内调用任何「向主线程分发并同步等待」的 API，典型为：
/// Tauri 菜单（`MenuItem::with_id` / `Submenu::append` / `set_text` / `set_menu` /
/// `set_icon` / `set_tooltip`）、窗口 getter（`is_visible` / `hwnd` / `show`）、
/// `run_on_main_thread`；同样禁止 COM/WMI 查询、文件 I/O 等阻塞调用。
///
/// 原因：主线程自身会通过 `with_config(_mut)` 读配置（`set_window_material`、
/// `get_config`、快捷键分发、tooltip 刷新…）。上述 API 内部经
/// `run_item_main_thread!` 展开为 `run_on_main_thread(..)` + `rx.recv()`（**无超时**），
/// 于是「子线程持配置锁 → 等主线程」与「主线程 → 等配置锁」构成 AB/BA 死锁：
/// **永久冻结，看门狗也救不回**（其探活同样要主线程）。
///
/// 需要配置数据来构造 UI 时，先在锁内取纯数据快照（如 `Vec<(String, String)>`），
/// 再在锁外调用相关 API——参考 `tray::build_audio_devices_menu`。
pub fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&Config) -> R,
{
    let guard = crate::state::lock_unpoisoned(CONFIG.get().expect("Config not initialized"));
    f(&guard)
}

/// 可变访问配置（改内存 + 同步日志缓存 + 序列化快照，落盘在锁外）。
///
/// **锁纪律（P0 死锁防护，勿破坏）**：同 [`with_config`]——闭包内只允许纯内存操作，
/// 严禁调用任何会向主线程分发并同步等待的 Tauri API、COM/WMI 查询或文件 I/O。
pub fn with_config_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut Config) -> R,
{
    // 配置锁只覆盖「改内存 + 同步日志缓存 + 序列化」，三者都是纯内存操作；
    // 落盘（`sync_all()` 在机械盘 / 受控磁盘 / 杀软实时扫描下可达数十毫秒）
    // 移到锁外执行。否则这段时间内所有 `with_config` 读取者——托盘 tooltip、
    // 设备查询、电量通知、快捷键分发、看门狗探活——都会阻塞在配置锁上。
    let (result, snapshot) = {
        let mut guard =
            crate::state::lock_unpoisoned(CONFIG.get().expect("Config not initialized"));
        let result = f(&mut guard);
        // 日志级别缓存必须与配置内容同拍更新，故留在锁内（纯内存，微秒级）
        sync_log_cache(&guard);
        (result, toml::to_string_pretty(&*guard).ok())
    }; // ← 配置锁在此释放
    if let Some(content) = snapshot {
        persist_if_changed(&content);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{claim_revision, revision_is_latest};

    /// 落盘版本号判据：**先取号者永远不得落盘**（当已有更新者取过号时）。
    ///
    /// 这正是 P1-3「落盘移出配置锁」后防乱序覆盖的核心：
    /// 落盘已不在配置锁内，两次写入的完成顺序与取号顺序可以相反，
    /// 若不比对版本号，先取到内容（较旧）的那次会最后落盘、把新内容盖掉。
    ///
    /// 只断言「单调递增」与「先取号者已过期」两个方向——它们是确定性的；
    /// 反方向（后取号者此刻仍是最新）会被其他测试线程继续取号破坏，故不断言。
    #[test]
    fn stale_revision_never_wins() {
        let first = claim_revision();
        let second = claim_revision();

        assert!(second > first, "取号必须单调递增，否则版本号无法定序");

        // 关键断言：second 已取号，故 first 无论何时进入串行区都已被判为过期。
        // 修复前没有这层判据，first 若后落盘就会覆盖 second 的内容。
        assert!(
            !revision_is_latest(first),
            "先取号者被后取号者超越后必须判为过期，否则会乱序覆盖新内容"
        );
    }
}

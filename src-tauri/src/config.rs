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
    #[serde(default)]
    pub device_shortcuts: std::collections::HashMap<String, DeviceShortcut>,
    #[serde(default)]
    pub enable_device_shortcut_cycle: bool,
    #[serde(default = "default_theme_mode")]
    pub theme_mode: String,
    #[serde(default = "default_window_material")]
    pub window_material: String,
    #[serde(default)]
    pub enable_24g_battery: bool,
}

fn default_true() -> bool {
    true
}
fn default_popup_tab() -> String {
    "devices".to_string()
}
fn default_theme_mode() -> String {
    "follow_system".to_string()
}
fn default_window_material() -> String {
    "default".to_string()
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
            device_shortcuts: std::collections::HashMap::new(),
            enable_device_shortcut_cycle: false,
            theme_mode: default_theme_mode(),
            window_material: default_window_material(),
            enable_24g_battery: false,
        }
    }
}

impl Config {
    /// Combined regex for all device exclusion filters (case-insensitive)
    fn default_filter_regex() -> String {
        "Virtual|虚拟|^HID|Audio Device|Audio 设备|Hands-Free|A2DP|gvinput Device|英特尔\\(R\\)"
            .to_string()
    }
}

static CONFIG: OnceLock<Mutex<Config>> = OnceLock::new();
/// 日志级别进程缓存：0=关闭 1=标准 2=详细
static LOG_LEVEL: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);
static LOG_ONCE: AtomicBool = AtomicBool::new(false);

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
                    crate::process::append_log(&format!("[config] parse error: {}", e));
                    Config::default()
                }
            },
            Err(e) => {
                eprintln!("[config] load failed (using defaults): {}", e);
                crate::process::append_log(&format!(
                    "[config] load failed (using defaults): {}",
                    e
                ));
                Config::default()
            }
        }
    };
    {
        let mut guard = crate::state::lock_unpoisoned(CONFIG.get().unwrap());
        *guard = config;
        sync_log_cache(&guard);
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
            crate::process::append_log(&format!(
                "[config] 旧版日志开关已迁移为级别: {}",
                c.log_level
            ));
        });
    }
}

pub fn with_config<F, R>(f: F) -> R
where
    F: FnOnce(&Config) -> R,
{
    let guard = crate::state::lock_unpoisoned(CONFIG.get().expect("Config not initialized"));
    f(&guard)
}

pub fn with_config_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut Config) -> R,
{
    let mut guard = crate::state::lock_unpoisoned(CONFIG.get().expect("Config not initialized"));
    let result = f(&mut guard);
    if let Ok(content) = toml::to_string_pretty(&*guard) {
        use std::io::Write;
        if let Err(e) =
            std::fs::File::create(&config_path()).and_then(|mut f| f.write_all(content.as_bytes()))
        {
            crate::process::append_log(&format!("[config] save failed: {}", e));
        }
    }
    sync_log_cache(&guard);
    result
}

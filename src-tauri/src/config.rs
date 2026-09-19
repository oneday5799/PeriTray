use crate::standard_log;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Mutex, OnceLock};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LogRetention {
    Once,
    /// 默认保留期。`Default` 由 derive 生成，与下方 `Deserialize` 的未知值降级
    /// 共用同一处语义（两者都指向 `OneDay`）。
    #[default]
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
            // 未知取值降级为默认，而不是让整份 Config 反序列化失败（P3-9）。
            //
            // 为什么不能返回 Err：`#[serde(default)]` **只在字段缺失时生效**，
            // 字段存在但值非法时错误会一路上抛 ⇒ 整份 `Config` 解析失败 ⇒
            // `init_config` 回退 `Config::default()`，用户全部个性化配置被一次性
            // 抹掉（P1-7 修的就是这条不可逆路径）。配置是用户数据，一个字段的
            // 取值无法识别不应牵连其余字段。
            //
            // 降级后由 `normalize_config` 在写盘前把该字段稳定成 `one_day`，
            // 避免未知值被原样持久化、下次启动再走一遍同样的分支。
            _ => Ok(Self::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceShortcut {
    pub name: String,
    pub shortcut: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    // ── 字段级 serde 默认值（P1-7）─────────────────────────────
    // 每个字段都必须有默认值：任何一个字段缺失或类型不符，都会让**整份**
    // `Config` 反序列化失败，进而回退到 `Config::default()`——用户全部个性化
    // 配置一次性丢失（唯一不可逆项）。补默认值时**逐字段比对
    // `Config::default()`**，绝不可一律写裸 `#[serde(default)]`：
    // 裸默认给的是「零值」，而下列字段的真实默认值是**非零值**。
    #[serde(default)]
    pub auto_start: bool,
    #[serde(default)]
    pub hidden_devices: Vec<String>,
    /// ⚠️ 具名 helper：真实默认是隐藏 `Battery` / `Monitor` 两组，
    /// 裸 `#[serde(default)]` 会得到空数组 ⇒ 升级后默认隐藏的分组突然出现。
    #[serde(default = "default_hidden_groups")]
    pub hidden_groups: Vec<String>,
    #[serde(default)]
    pub device_names: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub device_groups: std::collections::HashMap<String, String>,
    #[serde(default = "default_true")]
    pub filter_enabled: bool,
    /// ⚠️ 具名 helper：真实默认是内置过滤正则，
    /// 裸 `#[serde(default)]` 会得到空串 ⇒ 设备过滤被静默整体关闭。
    #[serde(default = "default_filter_regex")]
    pub filter_regex: String,
    #[serde(default = "default_true")]
    pub dedup_devices: bool,
    #[serde(default)]
    pub show_unnamed_bt: bool,
    #[serde(default)]
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
/// `hidden_groups` 的 serde 默认值。与 `Config::default()` 共用同一份定义，
/// 避免「字段缺失」与「整体默认」两条路径给出不同的默认隐藏分组（P1-7）。
fn default_hidden_groups() -> Vec<String> {
    vec!["Battery".to_string(), "Monitor".to_string()]
}
/// `filter_regex` 的 serde 默认值（复用 `Config::default_filter_regex`，单一来源）
fn default_filter_regex() -> String {
    Config::default_filter_regex()
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

/// 各「字符串枚举」字段的合法取值（**单一来源**）——取自前端下拉框的
/// `data-value` 集合（`settings.html`）。改前端选项时必须同步这里，
/// 否则新选项会被归一化回默认值（表现为「选了没生效」）。
const VALID_LOG_LEVELS: &[&str] = &["off", "standard", "verbose"];
const VALID_POPUP_TABS: &[&str] = &["devices", "volume"];
const VALID_POPUP_SIZES: &[&str] = &["small", "default", "large"];
const VALID_THEME_MODES: &[&str] = &["follow_system", "light", "dark"];
const VALID_WINDOW_MATERIALS: &[&str] = &["default", "acrylic", "mica"];

/// 低电量阈值个数上限（与前端 `settings-devices.js` 的「最多5个阈值」一致）
const MAX_BATTERY_THRESHOLDS: usize = 5;
/// 电量刷新间隔的合法区间（秒），与前端 blur 校验的「须为10-3600的整数」一致。
/// 注意 `tray.rs` 读取时只用 `.max(10)` 钳制了下界，上界原本无兜底。
const MIN_BATTERY_REFRESH_SECS: u32 = 10;
const MAX_BATTERY_REFRESH_SECS: u32 = 3600;

/// 把 `value` 收敛到 `allowed` 内；非法时替换为 `fallback`。
/// 返回是否发生了替换（供调用方决定要不要记日志）。
fn normalize_choice(value: &mut String, allowed: &[&str], fallback: &str) -> bool {
    if allowed.contains(&value.as_str()) {
        return false;
    }
    // 原地改写而不是 `*value = fallback.to_string()`：保留已有容量，避免多一次分配
    value.clear();
    value.push_str(fallback);
    true
}

/// 低电量阈值集合的合法性：1~5 个、每个在 0~100、互不重复。
/// 四条与前端 blur 校验逐条对应（`parts.length === 0` / `> 5` /
/// `n < 0 || n > 100` / `new Set(nums).size !== nums.length`）。
fn battery_thresholds_valid(thresholds: &[i32]) -> bool {
    if thresholds.is_empty() || thresholds.len() > MAX_BATTERY_THRESHOLDS {
        return false;
    }
    thresholds.iter().all(|v| (0..=100).contains(v))
        && thresholds
            .iter()
            .enumerate()
            .all(|(i, v)| !thresholds[..i].contains(v))
}

/// 集中归一化：把「可从 `config.toml` / 前端直接写入」的字段收敛到应用支持的取值集合。
///
/// **为什么需要它**：`log_level` / `popup_size` / `theme_mode` / `window_material` /
/// `default_popup_tab` 在 `Config` 里是自由 `String`，非法值会被**原样持久化**，
/// 之后又在各处被各自解析（`popup_size_dims` 的 `_ => (360.0, 520.0)`、
/// `parse_log_level` 的 `_ => 0`、`check_material_support` 的 `_ => false`…）
/// ⇒ 同一个非法值在不同调用点有不同兜底行为，且磁盘上长期留着脏数据。
/// 这里把校验收敛成单一来源。
///
/// **为什么是「归一化」而不是「报错」**：这些值来自用户可直接编辑的 `config.toml`。
/// 报错会让一个字段牵连整份配置（P1-7 那条不可逆路径），归一化则只影响该字段本身。
///
/// 纯函数：不读全局状态、不持锁、不做 I/O ⇒ 可被加载路径与写入路径复用，也便于单测。
/// 返回 `true` 表示至少有一个字段被替换。
fn normalize_config(config: &mut Config) -> bool {
    let mut changed = false;

    changed |= normalize_choice(&mut config.log_level, VALID_LOG_LEVELS, "off");
    changed |= normalize_choice(&mut config.default_popup_tab, VALID_POPUP_TABS, "devices");
    changed |= normalize_choice(&mut config.popup_size, VALID_POPUP_SIZES, "default");
    changed |= normalize_choice(&mut config.theme_mode, VALID_THEME_MODES, "follow_system");
    changed |= normalize_choice(
        &mut config.window_material,
        VALID_WINDOW_MATERIALS,
        "default",
    );

    if !battery_thresholds_valid(&config.low_battery_thresholds) {
        config.low_battery_thresholds = default_battery_thresholds();
        changed = true;
    }

    if !(MIN_BATTERY_REFRESH_SECS..=MAX_BATTERY_REFRESH_SECS)
        .contains(&config.low_battery_refresh_secs)
    {
        config.low_battery_refresh_secs = default_battery_refresh_secs();
        changed = true;
    }

    changed
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_start: false,
            hidden_devices: vec![],
            hidden_groups: default_hidden_groups(),
            device_names: std::collections::HashMap::new(),
            device_groups: std::collections::HashMap::new(),
            filter_enabled: true,
            filter_regex: default_filter_regex(),
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

/// 配置锁。**锁序登记见 `state.rs` 模块文档**（本锁在其中的层级、允许/禁止的嵌套边）。
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

/// 启动时配置解析失败的原因（含备份路径）。只在 `init_config` 写入一次，供前端提示。
/// 非清除式：popup 与 settings 两个窗口都会读，读到的是同一条信息。
static CONFIG_LOAD_ERROR: OnceLock<Mutex<Option<String>>> = OnceLock::new();

fn set_load_error(msg: String) {
    let slot = CONFIG_LOAD_ERROR.get_or_init(|| Mutex::new(None));
    *crate::state::lock_unpoisoned(slot) = Some(msg);
}

/// 供前端查询的「启动期配置错误」。`None` 表示本次启动读取正常。
pub fn get_load_error() -> Option<String> {
    CONFIG_LOAD_ERROR
        .get()
        .and_then(|slot| crate::state::lock_unpoisoned(slot).clone())
}

/// 解析失败时把磁盘原文另存为 `config.toml.bak`。
///
/// 为什么必须备份：解析失败后进程内是默认值，而**后续任意一次写入都会用默认值
/// 覆盖 config.toml**——原始配置会被永久销毁。备份是这条不可逆路径上唯一的救生索。
/// 备份失败不阻断启动，但要把「无法备份」写进给用户的提示里。
fn backup_broken_config(path: &std::path::Path) -> Option<std::path::PathBuf> {
    let bak = path.with_extension("toml.bak");
    match std::fs::copy(path, &bak) {
        Ok(_) => Some(bak),
        Err(e) => {
            eprintln!("[config] 备份损坏配置失败: {}", e);
            None
        }
    }
}

/// 从磁盘原文构造进程内配置：**解析 → 归一化**（P3-9）。
///
/// 抽成独立函数是为了让单测能用**任意文本**验证「一个坏字段不会牵连整份配置」，
/// 而不必去碰真实的 `config.toml`。返回的 `bool` 表示是否发生了归一化替换。
///
/// 归一化放在**解析之后、交给进程之前**：这样「首次读取」就已经是干净值，
/// 不会让脏值先经 `get_config` 发给前端。
fn parse_config_text(text: &str) -> Result<(Config, bool), toml::de::Error> {
    let mut config: Config = toml::from_str(text)?;
    let normalized = normalize_config(&mut config);
    Ok((config, normalized))
}

pub fn init_config() {
    CONFIG.set(Mutex::new(Config::default())).ok();
    // 归一化结果要延后到日志级别缓存建立之后再上报，见下方注释。
    let mut normalized_on_load = false;
    let config = {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => match parse_config_text(&content) {
                Ok((config, normalized)) => {
                    normalized_on_load = normalized;
                    config
                }
                Err(e) => {
                    // 解析失败：**先备份磁盘原文，再回退默认值**——默认值一旦被后续
                    // 写入落盘，原始配置就永久消失（P1-7 的唯一不可逆路径）。
                    let bak = backup_broken_config(&path);
                    let hint = match &bak {
                        Some(p) => format!("原配置已备份为 {}，可据此手工恢复", p.display()),
                        None => "原配置备份失败，请先手工复制 config.toml 再改设置".to_string(),
                    };
                    // stderr 直出：日志门控依赖本文件解析成功，失败时必须可见
                    eprintln!("[config] parse error: {}（{}）", e, hint);
                    standard_log!("[config] parse error: {}（{}）", e, hint);
                    set_load_error(format!("配置文件解析失败，已恢复默认设置。{}", hint));
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
    // 「载入时归一化」必须记在**日志级别缓存建立之后**。
    //
    // 踩过的坑：这一行原先写在解析分支里（即 `sync_log_cache` 之前），
    // 而那时 `LOG_LEVEL` 还是静态初值 0 ⇒ `standard_log_enabled()` 判为关闭，
    // 这行日志**永远不会输出**（等于死代码）。归一化发生在日志缓存之前，
    // 是它天然会踩到的时间差。
    //
    // 仍然受用户配置的 `log_level` 门控：归一化是**修复**而非数据丢失，
    // 用户主动把日志关掉时不打扰他（对照：解析失败那条必须强开日志）。
    if normalized_on_load {
        standard_log!("[config] 载入时归一化：存在非法字段，已回退为默认值");
    }
    // 解析失败时强制开启标准级日志，并把错误补写进日志文件。
    //
    // 为什么必须强制：回退用的 `Config::default()` 里 `log_level` 是 "off"
    // （见 `default_log_level`），于是**恰恰在最需要现场的时候，日志目录里空无一物**；
    // 安装版没有控制台，上面那句 `eprintln!` 也无处可见。若不强制打开，
    // 「配置为什么坏了」将没有任何可查的证据（只剩前端一条提示）。
    // 注意：仅本次运行生效，不写盘、不改用户配置。
    if let Some(err) = get_load_error() {
        LOG_LEVEL.store(1, Ordering::Relaxed);
        standard_log!("[config] {}", err);
    }
    // 初始化脏检查缓存：读取磁盘文件内容作为基准
    if let Ok(content) = std::fs::read_to_string(config_path()) {
        if let Some(last) = LAST_CONFIG_CONTENT.get() {
            // P2-11：改用统一入口，中毒时不再静默丢弃基准值
            *crate::state::lock_unpoisoned(last) = Some(content);
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

/// 测试专用：确保全局 `CONFIG` 已初始化，**不读磁盘**。
///
/// `OnceLock` 幂等，多个用例重复调用无妨。给那些「会经由 `with_config` 读配置、
/// 但不想依赖真实 `config.toml`」的用例用（如 `battery_notify` 的 P2-7 探针用例）。
/// 取 `Config::default()`：其中 `low_battery_devices` 为空 ⇒ 通知路径会提前返回，
/// 用例不会真的弹系统通知。
#[cfg(test)]
pub(crate) fn ensure_config_ready() {
    CONFIG.get_or_init(|| Mutex::new(Config::default()));
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

/// 一次待落盘的配置快照。
struct PersistJob {
    /// 入队时取的版本号（见 [`claim_revision`]）
    rev: u64,
    content: String,
}

/// 落盘队列：`with_config_mut` 只把快照交给写线程，调用线程立即返回（B11）。
///
/// **为什么要有它**：原实现是在**调用线程**上直接落盘（`File::create` +
/// `write_all` + `sync_all` + `rename`）。P1-3 已把落盘移出**配置锁**（那一步是对的），
/// 但没移出**调用线程** —— 而 11 个调用点里有 10 个就在**主线程**上：
/// 9 个同步命令（`update_config` / `toggle_device_hidden` / `rename_device` /
/// `change_device_group` / `toggle_group_hidden` / `toggle_audio_device_hidden` /
/// `set_hotkey_config` / `set_device_shortcut` / `remove_device_shortcut`）
/// 与 1 个托盘菜单事件（`tray.rs` 的「开机自启」勾选）。
/// `sync_all()` 在受控磁盘 / 杀软实时扫描下可达数十毫秒（本机 `append_log`
/// 单行落盘实测约 21ms，`sync_all` 只会更贵）⇒ 表现是「**改一次设置卡一下 UI**」。
///
/// 队列满时**退回同步落盘**而不是丢弃：配置是用户数据，丢一次设置比卡一下更糟。
///
/// ⚠️ **代价（知情，已评估）**：进程**异常终止**（崩溃 / 被强杀）时，最后一次设置
/// 可能尚未落盘。正常退出路径全部会 [`flush_persist`]（`RunEvent::Exit`、
/// 看门狗自重启前、`builder.build()` 失败后），故该窗口只存在于异常终止，
/// 通常 < 10ms。取舍理由：原实现是「**每次**改设置都卡 UI」（必然、高频），
/// 本实现是「**极端**情况下丢最后一次设置」（偶发、低损）。
static PERSIST_TX: OnceLock<SyncSender<PersistJob>> = OnceLock::new();

/// 已入队 / 已**处理完**的任务数，供 [`flush_persist`] 判断队列是否排空。
///
/// 注意「处理完」≠「落盘」：若任务在写线程取到它之前就已被更新的写入超越，
/// `persist_now` 会判其过期而**直接跳过**（这正是防乱序覆盖的机制），
/// 此时计数照加但不产生磁盘写。故契约是「**入队数 == 处理数**」——
/// 每条任务都被处置过（写盘或明确判弃），不会无声消失。
static PERSIST_QUEUED: AtomicU64 = AtomicU64::new(0);
static PERSIST_DONE: AtomicU64 = AtomicU64::new(0);

/// 队列容量。落盘是低频操作（用户改设置才触发），64 足以吸收任何突发；
/// 真满了会退回同步落盘，不会丢数据。
const PERSIST_QUEUE_CAP: usize = 64;

fn persist_sender() -> &'static SyncSender<PersistJob> {
    PERSIST_TX.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::sync_channel::<PersistJob>(PERSIST_QUEUE_CAP);
        std::thread::Builder::new()
            .name("peritray-config".to_string())
            .spawn(move || {
                while let Ok(job) = rx.recv() {
                    persist_now(&job);
                    PERSIST_DONE.fetch_add(1, Ordering::SeqCst);
                }
            })
            .expect("无法启动配置写线程");
        tx
    })
}

/// 把一份快照交给写线程；队列满或写线程已退出时**同步落盘兜底**（永不丢弃）。
fn enqueue_persist(content: String) {
    let job = PersistJob {
        rev: claim_revision(),
        content,
    };
    PERSIST_QUEUED.fetch_add(1, Ordering::SeqCst);
    match persist_sender().try_send(job) {
        Ok(()) => {}
        // 队列满 ⇒ 同步落盘。不丢弃：这是用户数据。
        Err(TrySendError::Full(job)) => {
            standard_log!("[config] 落盘队列已满，退回同步写");
            persist_now(&job);
            PERSIST_DONE.fetch_add(1, Ordering::SeqCst);
        }
        // 写线程已退出（只可能发生在进程收尾阶段）⇒ 同步兜底
        Err(TrySendError::Disconnected(job)) => {
            persist_now(&job);
            PERSIST_DONE.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// 等待落盘队列排空（上限 2s）。**正常退出路径必须调用**，
/// 否则关停前最后一次设置会留在队列里——这正是 B11 的已知代价，
/// 调用它把窗口收窄到「异常终止」这一种情况。
///
/// 上限是必需的：写线程可能正卡在一次极慢的 `sync_all()` 上，
/// 退出路径不能为此无限等待（用户点了退出就得退）。
/// 排不空时**记日志而不是静默返回**——静默会让「设置丢了」无从追查。
pub fn flush_persist() {
    let pending = PERSIST_QUEUED
        .load(Ordering::SeqCst)
        .saturating_sub(PERSIST_DONE.load(Ordering::SeqCst));
    if pending == 0 {
        // 常见路径（队列本就空）直接返回，不打扰日志
        return;
    }
    standard_log!("[config] flush_persist: 等待 {} 条落盘完成", pending);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while PERSIST_DONE.load(Ordering::SeqCst) < PERSIST_QUEUED.load(Ordering::SeqCst) {
        if std::time::Instant::now() >= deadline {
            standard_log!(
                "[config] flush_persist 超时：已写 {} / 已入队 {}（最后一次设置可能未落盘）",
                PERSIST_DONE.load(Ordering::SeqCst),
                PERSIST_QUEUED.load(Ordering::SeqCst)
            );
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    standard_log!("[config] flush_persist: 队列已排空");
}

/// 落盘一份快照：先做版本号与脏检查，再原子写入。**只在写线程上执行**
/// （`enqueue_persist` 的兜底分支是唯一例外，那时写线程已不可用）。
fn persist_now(job: &PersistJob) {
    // 串行锁**故意**跨 I/O 持有：保证两次落盘不交错（各写一半 = 半个文件）。
    // 且本函数已只在写线程上执行（`enqueue_persist` 的兜底分支除外），长时间持锁不影响 UI。
    //
    // 锁序（P3-10，集中登记见 `state.rs` 模块文档）：本函数在持 `PERSIST_LOCK` 时
    // 取 `LAST_CONFIG_CONTENT`（下面两处），即白名单第 2 条
    // `PERSIST_LOCK → LAST_CONFIG_CONTENT`。**反向边不存在**——没有任何路径在持
    // `LAST_CONFIG_CONTENT` 时取 `PERSIST_LOCK`，故不构成 AB/BA。
    // （早先这里写的是「不与任何其他锁构成嵌套」，与下面的事实不符，已改正。）
    let _serial = crate::state::lock_unpoisoned(&PERSIST_LOCK);

    // 已有更新的写入取过号 → 本次内容已过期，丢弃（防乱序覆盖）
    if !revision_is_latest(job.rev) {
        return;
    }

    // #23 脏检查：内容未变化时跳过写盘（减少高频配置操作的 I/O）
    //
    // ⚠️ 守卫必须收在块内，不能提升到函数作用域：本函数末尾（写盘成功后）
    // 还要**再取一次同一把锁**，Mutex 不可重入 ⇒ 同线程重复加锁 = 直接死锁。
    let last = LAST_CONFIG_CONTENT.get_or_init(|| Mutex::new(None));
    let unchanged = {
        let cached = crate::state::lock_unpoisoned(last);
        cached.as_deref() == Some(job.content.as_str())
    };
    if unchanged {
        return;
    }

    match write_config_atomically(&job.content, &config_path()) {
        // 写盘成功，更新缓存
        Ok(()) => *crate::state::lock_unpoisoned(last) = Some(job.content.clone()),
        Err(e) => standard_log!("[config] save failed: {}", e),
    }
}

/// 原子写入：先写临时文件并 `sync_all`，再 rename 替换（同卷原子操作）；
/// 失败时清理临时文件。抽成独立函数是为了让单测用**临时路径**验证，
/// 不必碰真实的 `config.toml`。
fn write_config_atomically(content: &str, cfg_path: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    let tmp_path = cfg_path.with_extension("toml.tmp");
    let result = std::fs::File::create(&tmp_path)
        .and_then(|mut f| {
            f.write_all(content.as_bytes())?;
            f.sync_all()?;
            Ok(())
        })
        .and_then(|_| std::fs::rename(&tmp_path, cfg_path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp_path);
    }
    result
}

/// `Config` 的字段清单（**单一来源**）：`merge_config` 与其覆盖性单测都由它生成，
/// 新增字段时**只需**在这里加一个标识符，两处自动同步。
///
/// ⚠️ 漏加字段的后果：该字段在设置页**永远改不动**（前端发来的整份配置里，
/// 它的差异不会被套用）。这是「可见的功能缺失」而非「静默丢数据」，
/// 且覆盖性单测（`merge_field_list_covers_every_serialized_field`）会直接转红。
macro_rules! for_each_config_field {
    ($mac:ident) => {
        $mac! {
            auto_start,
            hidden_devices,
            hidden_groups,
            device_names,
            device_groups,
            filter_enabled,
            filter_regex,
            dedup_devices,
            show_unnamed_bt,
            use_system_bt,
            wireless_only,
            tray_devices,
            hidden_audio_devices,
            log_level,
            legacy_log_enabled,
            log_retention,
            shutdown_volume_enabled,
            shutdown_volume_devices,
            mute_lock,
            volume_fine_adjust,
            force_mute_devices,
            enable_spatial_sound,
            check_updates,
            include_prerelease,
            simplify_device_names,
            shortcut_devices,
            shortcut_volume,
            shortcut_volume_up,
            shortcut_volume_down,
            shortcut_volume_mute,
            hardware_acceleration,
            default_popup_tab,
            popup_size,
            device_shortcuts,
            enable_device_shortcut_cycle,
            shortcut_switch_notify,
            theme_mode,
            window_material,
            low_battery_notify,
            low_battery_devices,
            low_battery_thresholds,
            low_battery_refresh_secs,
        }
    };
}

macro_rules! merge_config_impl {
    ($($field:ident),* $(,)?) => {
        /// 把「`patch` 相对 `base` 的差异」套用到 `current` 上，返回被套用的字段数（P1-11）。
        ///
        /// **为什么需要它**：设置页的 `saveConfig()` 发送**整份**配置并整体覆盖，
        /// 而弹窗侧的改名/隐藏/分组/托盘固定等操作走的是**字段级**命令
        /// （`rename_device` / `toggle_device_hidden` / `change_device_group` / …）。
        /// 设置页手里的快照一旦早于那些操作，整份覆盖就会把它们一并抹掉——
        /// 典型 lost update：**改名的同时切一个开关，名字被改回去**。
        ///
        /// **语义**：只有 `base` 与 `patch` 不同的字段才算「用户改了」，其余一律保留
        /// `current`（后端真值）。于是
        /// - 用户没碰过的字段：**结构性免疫**并发覆盖（不是靠时序侥幸）；
        /// - 同一字段被两边同时改：退化为 last-writer-wins——这既不可消除，
        ///   也不需要消除（后端无从得知谁更晚，而前端的改动是用户刚做的动作）。
        ///
        /// `base` 由前端随请求一起送来（它上次从后端收到的快照）。`base` 缺失时
        /// 调用方应退回整体覆盖并**打告警日志**（见 `commands::update_config`）。
        pub fn merge_config(current: &mut Config, base: &Config, patch: &Config) -> usize {
            let mut applied = 0usize;
            $(
                if base.$field != patch.$field {
                    current.$field = patch.$field.clone();
                    applied += 1;
                }
            )*
            applied
        }
    };
}
for_each_config_field!(merge_config_impl);

macro_rules! config_field_names_impl {
    ($($field:ident),* $(,)?) => {
        /// `merge_config` 覆盖的字段名（由 [`for_each_config_field`] 自动导出）。
        /// 只给覆盖性单测对账用——手写清单会漂。
        #[cfg(test)]
        pub(crate) const MERGED_FIELD_NAMES: &[&str] = &[$(stringify!($field)),*];
    };
}
for_each_config_field!(config_field_names_impl);

// ── B8：P0-4 类死锁的机械防线（debug-only）──────────────────────
//
// 为什么需要它：托盘/菜单 API（`set_menu` / `set_icon` / `set_tooltip` / `set_text`
// 及 `MenuItem::with_id` 等构造 API）内部经 `run_item_main_thread!` 展开为
// `run_on_main_thread(..) + rx.recv()`——**无超时地同步等主线程**；而主线程自身会通过
// `with_config(_mut)` 读配置。于是「持配置锁 → 调菜单 API」与「主线程 → 等该锁」
// 构成 **AB/BA 永久死锁**：整进程冻结，看门狗也救不回（其探活同样要主线程）。
//
// 这条纪律原先只写在 `AGENTS.md` 评审项与注释里，**没有任何机械防线**：
// `tools/check.mjs` 不扫 Rust，编译器也看不见。B8 把「持锁深度」记下来，
// 由 `tray.rs` 的薄包装在调用 API 前 `debug_assert!` —— 复发时开发期立刻 panic，
// 而不是线上冻结 40 秒后被系统按「无响应」杀掉。
//
// ⚠️ 深度必须记在**线程局部**里，不能用全局原子量：锁是线程级资源，
// 全局计数会让「A 线程持锁、B 线程调菜单」这种完全无害的组合误报。
#[cfg(debug_assertions)]
thread_local! {
    static CONFIG_LOCK_DEPTH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// 当前线程是否正持有配置锁。供 `tray.rs` 的薄包装做 `debug_assert!`。
#[cfg(debug_assertions)]
pub fn config_lock_held() -> bool {
    CONFIG_LOCK_DEPTH.with(|d| d.get() > 0)
}

/// release 版恒为 `false`。`debug_assert!` 在 release 下整块被编译掉、不会求值，
/// 保留这个同名函数只是为了让调用点在两种构建下都能编译（否则 `dead_code` 会报警）。
#[cfg(not(debug_assertions))]
pub fn config_lock_held() -> bool {
    false
}

/// 进出配置锁的深度守卫（B8）。用 RAII 而不是「进 +1 / 出 -1 两句」：
/// 闭包 `f` panic 时也能正确回退，否则一次 panic 会让计数永久偏高，
/// 此后**所有**断言都变成误报（比没有防线更糟）。
#[cfg(debug_assertions)]
struct ConfigLockDepthGuard;

#[cfg(debug_assertions)]
impl ConfigLockDepthGuard {
    fn enter() -> Self {
        CONFIG_LOCK_DEPTH.with(|d| d.set(d.get() + 1));
        Self
    }
}

#[cfg(debug_assertions)]
impl Drop for ConfigLockDepthGuard {
    fn drop(&mut self) {
        // saturating_sub：即便出现「多退一次」的编程错误，也不会回绕成天文数字
        CONFIG_LOCK_DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
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
    #[cfg(debug_assertions)]
    let _depth = ConfigLockDepthGuard::enter();
    f(&guard)
}

/// 写入路径的「归一化 → 同步日志缓存 → 序列化」三步（P3-9）。
///
/// 抽成独立函数是为了让**接线顺序**成为结构性保证而不是注释约定，并可被单测直接调用
/// （无需初始化全局 `CONFIG`）：
/// ① 先归一化，否则非法值会被原样写进 `config.toml`；
/// ② 再同步日志级别缓存，使其反映**最终**的 `log_level`；
/// ③ 最后序列化，快照必须是归一化之后的内容。
/// 三步都是纯内存操作（微秒级），故保留在配置锁内。
///
/// 返回 `(是否发生归一化, 待落盘快照)`。
fn finalize_before_persist(config: &mut Config) -> (bool, Option<String>) {
    let normalized = normalize_config(config);
    sync_log_cache(config);
    (normalized, toml::to_string_pretty(&*config).ok())
}

/// 可变访问配置（改内存 + 归一化 + 同步日志缓存 + 序列化快照，落盘交给写线程）。
///
/// **锁纪律（P0 死锁防护，勿破坏）**：同 [`with_config`]——闭包内只允许纯内存操作，
/// 严禁调用任何会向主线程分发并同步等待的 Tauri API、COM/WMI 查询或文件 I/O。
pub fn with_config_mut<F, R>(f: F) -> R
where
    F: FnOnce(&mut Config) -> R,
{
    // 配置锁只覆盖「改内存 + 归一化 + 同步日志缓存 + 序列化」，四者都是纯内存操作。
    // 落盘走两级外移，两级都是必需的：
    // ① **移出配置锁**（P1-3）：否则这段时间内所有 `with_config` 读取者——托盘 tooltip、
    //    设备查询、电量通知、快捷键分发、看门狗探活——都会阻塞在配置锁上；
    // ② **移出调用线程**（B11）：落盘交给 `peritray-config` 写线程，
    //    否则主线程上的 9 个同步命令与托盘菜单事件会各自卡一次 `sync_all()`。
    let (result, normalized, snapshot) = {
        let mut guard =
            crate::state::lock_unpoisoned(CONFIG.get().expect("Config not initialized"));
        // B8：debug 下登记「本线程正持配置锁」，供 `tray.rs` 的薄包装断言
        #[cfg(debug_assertions)]
        let _depth = ConfigLockDepthGuard::enter();
        let result = f(&mut guard);
        let (normalized, snapshot) = finalize_before_persist(&mut guard);
        (result, normalized, snapshot)
    }; // ← 配置锁在此释放
    if normalized {
        // 放在锁外：日志虽已异步化（B5），也没必要让它落在配置锁的临界区内。
        // 走到这里说明**前端送来了应用不支持的取值**（本应在前端就被拦住），
        // 属契约被破坏，故用标准级而非详细级记录。
        standard_log!("[config] 写入时归一化：存在非法字段，已回退为默认值");
    }
    if let Some(content) = snapshot {
        enqueue_persist(content);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::{
        claim_revision, config_lock_held, config_path, default_battery_refresh_secs,
        default_battery_thresholds, enqueue_persist, finalize_before_persist, flush_persist,
        merge_config, normalize_config, parse_config_text, revision_is_latest, with_config,
        write_config_atomically, Config, MERGED_FIELD_NAMES, PERSIST_DONE, PERSIST_QUEUED,
    };
    use std::sync::atomic::Ordering;

    // ── B8：P0-4 防复发断言的判据 ────────────────────────────

    // 测试用的「确保 CONFIG 已初始化」已提升为 `super::ensure_config_ready()`
    // （`battery_notify` 的 P2-7 探针用例也要用同一份实现，避免两处各写一遍）。
    use super::ensure_config_ready;

    /// B8 的核心判据：`config_lock_held()` 必须精确反映「**本线程**是否正持有配置锁」。
    ///
    /// 可证伪性：把 `with_config` 里的 `ConfigLockDepthGuard::enter()` 删掉，
    /// 锁内的断言会立刻转红（`config_lock_held()` 恒为 false）；把守卫改成
    /// 「进 +1 / 出 -1 两句」则下面的 `#[should_panic]` 用例（panic 后回退）转红。
    #[test]
    fn config_lock_held_reflects_actual_lock_state() {
        ensure_config_ready();

        assert!(!config_lock_held(), "锁外必须为 false");

        with_config(|_| {
            assert!(config_lock_held(), "锁内必须为 true");
        });

        assert!(!config_lock_held(), "出锁后必须回到 false");
    }

    /// 判据必须是**线程局部**的：别的线程持锁不得让本线程误报。
    ///
    /// 可证伪性：把 `CONFIG_LOCK_DEPTH` 从 `thread_local!` 换成全局 `AtomicUsize`，
    /// 本用例转红——而那种误报会让「A 线程读配置、B 线程刷托盘」这种完全无害的
    /// 组合在开发期直接 panic，比没有防线更糟。
    #[test]
    fn config_lock_held_is_thread_local() {
        ensure_config_ready();

        // 子线程在**持有配置锁**的同时通知主线程去查判据
        let (tx, rx) = std::sync::mpsc::channel();
        let handle = std::thread::spawn(move || {
            with_config(|_| {
                tx.send(()).expect("主线程应仍在等待");
                // 等主线程查完判据再出锁
                std::thread::sleep(std::time::Duration::from_millis(50));
            });
        });

        rx.recv_timeout(std::time::Duration::from_secs(2))
            .expect("子线程应已进入配置锁");
        assert!(
            !config_lock_held(),
            "子线程持锁期间，本线程的判据必须仍为 false"
        );

        handle.join().expect("子线程不应 panic");
    }

    /// 守卫必须是 RAII：闭包 panic 后深度也要回退。
    ///
    /// 可证伪性：把 `ConfigLockDepthGuard` 换成「进入时 +1、返回后 -1」两句写法，
    /// panic 会让深度永久停在 1，此后**每次** `config_lock_held()` 都返回 true
    /// ——防线退化成「任何菜单调用都误报」，比没有防线更糟。本用例即转红。
    #[test]
    fn lock_depth_recovers_after_panic_in_closure() {
        ensure_config_ready();

        let caught = std::panic::catch_unwind(|| {
            with_config(|_: &Config| -> () {
                panic!("模拟闭包内 panic");
            });
        });
        assert!(caught.is_err(), "闭包 panic 应向上传播");

        // `Mutex` 此时已中毒，`with_config` 内部的统一入口会忽略中毒，仍可用
        let inside = with_config(|_| config_lock_held());
        assert!(inside, "重新持锁时应为 true");
        assert!(
            !config_lock_held(),
            "出锁后深度必须已回退——否则后续所有断言都会误报"
        );
    }

    /// B8 端到端等价验证：`tray.rs` 薄包装里的断言形态，在配置锁内必须 panic。
    ///
    /// 这里复现的是**同一判据**（`config_lock_held()` 是两处唯一的公共依赖），
    /// 不是复制实现：包装体里也只有 `debug_assert!(!config_lock_held(), …)`。
    #[test]
    #[should_panic(expected = "P0-4")]
    fn menu_style_assertion_panics_inside_config_lock() {
        ensure_config_ready();

        with_config(|_| {
            debug_assert!(
                !config_lock_held(),
                "P0-4：持配置锁时调用菜单 API，会与主线程构成 AB/BA 永久死锁"
            );
        });
    }

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

    /// P1-7 回归：**字段缺失时必须补成 `Config::default()` 的逐字段默认值，而不是零值**。
    ///
    /// 失效模式（本条要防的）：升级后旧 config.toml 里没有新字段 → 若该字段没写
    /// `#[serde(default)]`，**整份** `Config` 反序列化失败 → 全部个性化配置丢失；
    /// 若写成裸 `#[serde(default)]`，则「零值」会冒充默认值——
    /// `hidden_groups` 变空数组（默认隐藏的 Battery / Monitor 突然出现）、
    /// `dedup_devices` 变 false（设备去重被静默关闭）、`filter_regex` 变空串
    /// （设备过滤整体失效）。两者都不可接受，故本测试逐字段钉住。
    #[test]
    fn missing_fields_fall_back_to_field_defaults_not_zero_values() {
        // 模拟「旧版 config.toml + 手工删掉两行」：只写出部分字段
        let partial = "auto_start = true\n\
                       filter_enabled = false\n\
                       show_unnamed_bt = true\n\
                       device_names = { \"HID\\\\VID_1234\" = \"我的手柄\" }\n";
        let cfg: super::Config =
            toml::from_str(partial).expect("部分字段的配置必须能解析，否则升级即丢全部个性化配置");
        let d = super::Config::default();

        // ① 已写出的字段必须原样保留（这一条在修复前必然失败）
        assert!(
            cfg.auto_start,
            "已写出的 auto_start 必须保留；失败说明整份配置被回退成了默认值"
        );
        assert!(!cfg.filter_enabled, "已写出的 filter_enabled 必须保留");
        assert!(cfg.show_unnamed_bt, "已写出的 show_unnamed_bt 必须保留");
        assert_eq!(
            cfg.device_names.get("HID\\VID_1234").map(String::as_str),
            Some("我的手柄"),
            "已写出的 device_names 必须保留（自定义设备名丢失是用户直接可见的损失）"
        );

        // ② 缺失字段必须补「逐字段默认值」，而不是零值
        assert_eq!(
            cfg.hidden_groups, d.hidden_groups,
            "hidden_groups 缺失时必须等于 Config::default()（默认隐藏 Battery/Monitor），\
             裸 #[serde(default)] 会给出空数组"
        );
        assert_eq!(
            cfg.filter_regex, d.filter_regex,
            "filter_regex 缺失时必须等于内置过滤正则，裸 #[serde(default)] 会给出空串（过滤整体失效）"
        );
        assert_eq!(
            cfg.dedup_devices, d.dedup_devices,
            "dedup_devices 缺失时必须为 true，否则去重被静默关闭"
        );
        assert_eq!(cfg.log_level, d.log_level);
        assert_eq!(cfg.default_popup_tab, d.default_popup_tab);
        assert_eq!(cfg.popup_size, d.popup_size);
        assert_eq!(cfg.theme_mode, d.theme_mode);
        assert_eq!(cfg.window_material, d.window_material);
        assert_eq!(cfg.low_battery_thresholds, d.low_battery_thresholds);
        assert_eq!(cfg.low_battery_refresh_secs, d.low_battery_refresh_secs);
        assert_eq!(cfg.hidden_devices, d.hidden_devices);
        assert_eq!(cfg.use_system_bt, d.use_system_bt);

        // ③ 空文档（极端情况）也必须能解析为全默认值
        let empty: super::Config = toml::from_str("").expect("空配置必须能解析为全默认值");
        assert_eq!(empty.hidden_groups, d.hidden_groups);
        assert_eq!(empty.dedup_devices, d.dedup_devices);

        // ④ 写回磁盘的内容就是 `toml::to_string_pretty(&config)`（见 persist_if_changed），
        //    故这里直接断言「下一次写盘的内容」里缺失字段已被补成正确默认值——
        //    等价于手工验证里的「改一次设置 → 看 config.toml 是否补全」。
        let written = toml::to_string_pretty(&cfg).expect("解析结果必须可序列化（写盘用）");
        let reread: super::Config = toml::from_str(&written).expect("写盘内容必须可回读");
        assert_eq!(
            reread.hidden_groups, d.hidden_groups,
            "写回磁盘后 hidden_groups 必须是默认隐藏组，否则用户下次启动会看到本该隐藏的分组"
        );
        assert!(reread.dedup_devices, "写回磁盘后 dedup_devices 必须为 true");
        assert_eq!(reread.filter_regex, d.filter_regex);
        assert!(reread.auto_start, "写回后已写出的字段仍必须保留");
    }

    /// P1-7 回归：默认配置必须能**原样往返**（序列化 → 反序列化 → 序列化）。
    ///
    /// 这条是「逐字段补默认值」的兜底检查：任何字段的 serde 属性写错
    /// （拼错 helper 名、漏了 `skip_serializing_if`、枚举字符串不匹配），
    /// 往返后都会在这里露出差异。
    #[test]
    fn default_config_roundtrips_unchanged() {
        let cfg = super::Config::default();
        let once = toml::to_string_pretty(&cfg).expect("默认配置必须可序列化");
        let back: super::Config = toml::from_str(&once).expect("序列化结果必须可回读");
        let twice = toml::to_string_pretty(&back).expect("回读结果必须可再序列化");
        assert_eq!(
            once, twice,
            "默认配置往返后内容发生变化，说明有字段的 serde 属性不一致"
        );
    }

    // ── P3-9：单个非法字段不得牵连整份配置 + 集中归一化 ────────────

    /// P3-9 回归：**单个字段值非法时不得让整份配置失效**（与字段缺失区分开）。
    ///
    /// 失效模式（本条要防的）：`LogRetention` 的 `Deserialize` 曾对未知值直接返回 `Err`，
    /// 而 `#[serde(default)]` **只在字段缺失时生效** ⇒ 一个非法枚举值就会让**整份**
    /// `Config` 反序列化失败 ⇒ `init_config` 回退 `Config::default()`，
    /// 用户全部个性化配置一次性丢失（P1-7 修掉的那条不可逆路径被重新打开）。
    #[test]
    fn invalid_enum_value_does_not_kill_whole_config() {
        let text = "auto_start = true\n\
                    log_level = \"verbose\"\n\
                    log_retention = \"not_a_real_value\"\n";
        let cfg: super::Config =
            toml::from_str(text).expect("单个枚举字段取值非法不得让整份 Config 解析失败");

        // ① 同一份文件里的其它字段必须原样保留（修复前这里会整份回退成默认值）
        assert!(
            cfg.auto_start,
            "非法枚举值不得牵连其它字段（失败说明整份配置被回退成了默认值）"
        );
        assert_eq!(cfg.log_level, "verbose", "非法枚举值不得牵连其它字段");

        // ② 非法字段本身降级为默认值
        assert_eq!(
            cfg.log_retention,
            super::LogRetention::OneDay,
            "未知 log_retention 应降级为默认值 OneDay"
        );

        // ③ 写回磁盘的内容必须稳定：不能把未知值原样持久化，
        //    否则每次启动都要重新降级，且磁盘上长期留着脏数据。
        let written = toml::to_string_pretty(&cfg).expect("解析结果必须可序列化");
        assert!(
            written.contains("log_retention = \"one_day\""),
            "未知值必须被稳定成 one_day：{written}"
        );
    }

    /// P3-9：**加载路径**必须把非法字段归一化掉（而不只是「让反序列化别失败」）。
    ///
    /// 这条覆盖 `parse_config_text` 里「解析 → 归一化」这一步的真实接线：
    /// 若把归一化从加载路径删掉，非法值会原样进入进程内存并被 `get_config`
    /// 发给前端，本用例即转红。
    #[test]
    fn load_path_normalizes_invalid_values() {
        let text = "theme_mode = \"sepia\"\n\
                    popup_size = \"huge\"\n\
                    log_level = \"verbose\"\n\
                    auto_start = true\n";
        let (cfg, normalized) = parse_config_text(text).expect("非法字段值不得让整份配置解析失败");

        assert!(normalized, "加载路径应报告发生了归一化");
        assert_eq!(cfg.theme_mode, "follow_system", "非法主题模式应回退默认值");
        assert_eq!(cfg.popup_size, "default", "非法尺寸档位应回退默认值");
        assert_eq!(cfg.log_level, "verbose", "合法字段不得被改动");
        assert!(cfg.auto_start, "合法字段不得被改动");
    }

    /// P3-9：**写入路径**必须「先归一化、后序列化」。
    ///
    /// 覆盖 `finalize_before_persist` 的接线顺序：若把归一化挪到序列化之后（或删掉），
    /// 非法值会被原样写进 `config.toml`，本用例即转红。
    #[test]
    fn write_path_finalize_normalizes_before_serializing() {
        let mut cfg = Config {
            log_level: "trace".to_string(),
            theme_mode: "sepia".to_string(),
            window_material: "blur".to_string(),
            ..Default::default()
        };

        let (normalized, snapshot) = finalize_before_persist(&mut cfg);

        assert!(normalized, "存在非法值时必须报告已替换");
        assert_eq!(cfg.log_level, "off", "内存中的值必须已被替换");
        assert_eq!(cfg.window_material, "default");

        let text = snapshot.expect("配置必须可序列化");
        assert!(
            text.contains("theme_mode = \"follow_system\""),
            "落盘快照必须是归一化后的值：{text}"
        );
        for dirty in ["sepia", "blur", "trace"] {
            assert!(
                !text.contains(dirty),
                "落盘快照里不得残留非法值 {dirty}：{text}"
            );
        }
    }

    /// P3-9：非法值被归一化到默认值，且**只影响自身**、不触碰无关字段。
    #[test]
    fn normalize_config_replaces_invalid_values_field_by_field() {
        let mut cfg = Config {
            auto_start: true, // 无关字段：必须原样保留
            log_level: "trace".to_string(),
            default_popup_tab: "settings".to_string(),
            popup_size: "huge".to_string(),
            theme_mode: "sepia".to_string(),
            window_material: "blur".to_string(),
            low_battery_thresholds: vec![10, 10, 101],
            low_battery_refresh_secs: 9,
            ..Default::default()
        };
        cfg.device_names
            .insert("VID_1".to_string(), "我的鼠标".to_string());

        assert!(normalize_config(&mut cfg), "存在非法值时必须报告已替换");

        assert_eq!(cfg.log_level, "off");
        assert_eq!(cfg.default_popup_tab, "devices");
        assert_eq!(cfg.popup_size, "default");
        assert_eq!(cfg.theme_mode, "follow_system");
        assert_eq!(cfg.window_material, "default");
        assert_eq!(cfg.low_battery_thresholds, default_battery_thresholds());
        assert_eq!(cfg.low_battery_refresh_secs, default_battery_refresh_secs());

        assert!(cfg.auto_start, "归一化不得触碰无关字段");
        assert_eq!(
            cfg.device_names.get("VID_1").map(String::as_str),
            Some("我的鼠标"),
            "归一化不得触碰无关字段（用户自定义设备名丢失是直接可见的损失）"
        );
    }

    /// P3-9：**合法值必须原样保留**（含各区间的边界值）。
    ///
    /// 与上一条构成对照：只做上一条的话，「把所有值都改成默认值」的错误实现也能通过，
    /// 必须靠这一条把「合法值不被触碰」钉住。
    #[test]
    fn normalize_config_keeps_valid_values_untouched() {
        let mut cfg = Config {
            log_level: "verbose".to_string(),
            default_popup_tab: "volume".to_string(),
            popup_size: "large".to_string(),
            theme_mode: "dark".to_string(),
            window_material: "mica".to_string(),
            low_battery_thresholds: vec![100, 0, 50],
            low_battery_refresh_secs: 3600,
            ..Default::default()
        };
        let before = cfg.clone();

        assert!(!normalize_config(&mut cfg), "全合法时不应报告替换");
        assert_eq!(cfg, before, "合法配置归一化后必须逐字段相等");

        // 区间下边界：刷新间隔 10 秒合法（9 秒非法，见上一条用例）
        let mut lo = Config {
            low_battery_refresh_secs: 10,
            ..Default::default()
        };
        assert!(!normalize_config(&mut lo));
        assert_eq!(lo.low_battery_refresh_secs, 10, "下边界 10 秒必须被接受");

        // 默认配置本身必须全合法：否则每次启动都会「静默修正」一次自己的默认值
        let mut d = Config::default();
        assert!(
            !normalize_config(&mut d),
            "Config::default() 必须是归一化的不动点，否则默认值与前端口径不一致"
        );
    }

    /// P3-9：低电量阈值的四条约束逐条钉住（与前端 blur 校验一一对应）。
    #[test]
    fn normalize_config_battery_threshold_rules_match_frontend() {
        // ① 空数组非法（前端文案：「请输入至少一个阈值」）
        let mut empty = Config {
            low_battery_thresholds: vec![],
            ..Default::default()
        };
        assert!(normalize_config(&mut empty));
        assert_eq!(empty.low_battery_thresholds, default_battery_thresholds());

        // ② 超过 5 个非法（前端文案：「最多5个阈值」）
        let mut too_many = Config {
            low_battery_thresholds: vec![1, 2, 3, 4, 5, 6],
            ..Default::default()
        };
        assert!(normalize_config(&mut too_many));
        assert_eq!(
            too_many.low_battery_thresholds,
            default_battery_thresholds()
        );

        // ③ 越界非法（前端文案：「超出范围(0-100)」）——两侧都验
        let mut too_low = Config {
            low_battery_thresholds: vec![-1],
            ..Default::default()
        };
        assert!(normalize_config(&mut too_low));
        assert_eq!(too_low.low_battery_thresholds, default_battery_thresholds());

        let mut too_high = Config {
            low_battery_thresholds: vec![101],
            ..Default::default()
        };
        assert!(normalize_config(&mut too_high));
        assert_eq!(
            too_high.low_battery_thresholds,
            default_battery_thresholds()
        );

        // ④ 重复值非法（前端文案：「有重复值」）
        let mut dup = Config {
            low_battery_thresholds: vec![15, 15],
            ..Default::default()
        };
        assert!(normalize_config(&mut dup));
        assert_eq!(dup.low_battery_thresholds, default_battery_thresholds());

        // 恰好 5 个、含两端边界 ⇒ 合法
        let mut ok = Config {
            low_battery_thresholds: vec![0, 25, 50, 75, 100],
            ..Default::default()
        };
        assert!(!normalize_config(&mut ok));
        assert_eq!(ok.low_battery_thresholds, vec![0, 25, 50, 75, 100]);
    }

    // ── P1-11：整份覆盖 ⇒ 按差异合并 ────────────────────────────

    /// **P1-11 的原始症状**：设置页手里的快照早于弹窗的改名，
    /// 用户只切了一个开关，改名不能被抹掉。
    #[test]
    fn merge_preserves_concurrent_field_changes() {
        let base = Config::default(); // 设置页手里的旧快照
        let mut patch = base.clone(); // 用户只动了 auto_start
        patch.auto_start = !base.auto_start;

        let mut current = base.clone(); // 后端真值：弹窗刚改过 device_names
        current
            .device_names
            .insert("VID_1".to_string(), "我的鼠标".to_string());

        let applied = merge_config(&mut current, &base, &patch);

        assert_eq!(applied, 1, "只有 auto_start 一个字段被改动");
        assert_eq!(current.auto_start, patch.auto_start, "用户的改动要生效");
        assert_eq!(
            current.device_names.get("VID_1").map(String::as_str),
            Some("我的鼠标"),
            "并发的改名不能被整份覆盖抹掉（P1-11 的原始症状）"
        );
    }

    /// 前端一个字段都没改 ⇒ 不得触碰任何字段（结构性免疫并发覆盖）。
    #[test]
    fn merge_never_touches_unchanged_fields() {
        let base = Config::default();
        let patch = base.clone();

        let mut current = base.clone();
        current.filter_regex = "用户手改".to_string();
        current.hidden_groups = vec!["X".to_string()];

        let applied = merge_config(&mut current, &base, &patch);

        assert_eq!(applied, 0, "base 与 patch 相同 ⇒ 不应套用任何字段");
        assert_eq!(current.filter_regex, "用户手改");
        assert_eq!(current.hidden_groups, vec!["X".to_string()]);
    }

    /// 同一字段被两边同时改 ⇒ last-writer-wins，且**以 patch 为准**
    /// （后端的改动可能更晚，但前端的改动是用户刚刚做的动作）。
    #[test]
    fn merge_resolves_same_field_conflict_in_favor_of_patch() {
        let base = Config::default();
        let mut patch = base.clone();
        patch.log_level = "verbose".to_string();

        let mut current = base.clone();
        current.log_level = "off".to_string();

        let applied = merge_config(&mut current, &base, &patch);

        assert_eq!(applied, 1);
        assert_eq!(current.log_level, "verbose");
    }

    /// 多个字段同时改动时，只有这些字段被套用（用计数锁住「只套差异」这一语义）。
    #[test]
    fn merge_applies_exactly_the_changed_fields() {
        let base = Config::default();
        let mut patch = base.clone();
        patch.wireless_only = !base.wireless_only;
        patch.mute_lock = !base.mute_lock;
        patch.theme_mode = "dark".to_string();
        patch.low_battery_thresholds = vec![10, 20, 30];

        let mut current = base.clone();
        current.popup_size = "large".to_string(); // 并发改动，不在 patch 里

        let applied = merge_config(&mut current, &base, &patch);

        assert_eq!(applied, 4, "恰好 4 个字段有差异");
        assert_eq!(current.popup_size, "large", "未在 patch 中的字段保持真值");
        assert_eq!(current.theme_mode, "dark");
        assert_eq!(current.low_battery_thresholds, vec![10, 20, 30]);
    }

    /// **覆盖性守卫**：`merge_config` 的字段清单必须与 `Config` 的落盘字段集**双向相等**。
    ///
    /// 为什么按**字段名集合**而不是数个数：TOML 没有 null，`toml` crate 序列化时会
    /// **跳过 `None`**，所以 `Config::default()` 里有 6 个 `Option` 字段压根不出现在
    /// 结果里（`legacy_log_enabled` + 5 个 `shortcut_*`）。数个数就得硬编码偏移量，
    /// 而偏移量本身也会漂。
    ///
    /// ⚠️ **新增可选（`Option`）字段时**：请同时在下面的 `probe` 里把它设为 `Some`，
    /// 否则它默认不出现在序列化结果里，本用例覆盖不到它。
    #[test]
    fn merge_field_list_covers_every_serialized_field() {
        use std::collections::BTreeSet;

        // 把默认 `None` 的可选字段填上，使它们进入序列化结果（见上方 ⚠️）
        let probe = Config {
            legacy_log_enabled: Some(true),
            shortcut_devices: Some("A".to_string()),
            shortcut_volume: Some("B".to_string()),
            shortcut_volume_up: Some("C".to_string()),
            shortcut_volume_down: Some("D".to_string()),
            shortcut_volume_mute: Some("E".to_string()),
            ..Default::default()
        };

        let text = toml::to_string_pretty(&probe).expect("Config 应可序列化为 TOML");
        let serialized: BTreeSet<String> = toml::from_str::<toml::Table>(&text)
            .expect("序列化结果应可解析回 TOML 表")
            .into_iter()
            .map(|(k, _)| k)
            .collect();
        let merged: BTreeSet<String> = MERGED_FIELD_NAMES.iter().map(|s| s.to_string()).collect();

        let forgotten: Vec<&String> = serialized.difference(&merged).collect();
        assert!(
            forgotten.is_empty(),
            "以下字段会落盘但不在 merge_config 的清单里 —— 它们将在设置页**永远改不动**，\
             请补进 for_each_config_field：{forgotten:?}"
        );
        let stale: Vec<&String> = merged.difference(&serialized).collect();
        assert!(
            stale.is_empty(),
            "merge_config 的清单里有字段已不落盘（可能已从 Config 删除或改成了跳过序列化）：{stale:?}"
        );
    }

    // ── B11：落盘写线程化 ────────────────────────────────────────

    /// 临时目录（按 pid 命名，避免并行用例互相踩）
    fn b11_temp_dir() -> std::path::PathBuf {
        std::env::temp_dir().join(format!("peritray-b11-{}", std::process::id()))
    }

    /// 原子写入：首次创建 + 覆盖替换 + 不残留临时文件。
    #[test]
    fn write_config_atomically_creates_then_replaces() {
        let dir = b11_temp_dir();
        std::fs::create_dir_all(&dir).expect("应能创建临时目录");
        let path = dir.join("config.toml");

        write_config_atomically("first", &path).expect("首次写入应成功");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        write_config_atomically("second", &path).expect("覆盖写入应成功");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "second");
        assert!(
            !path.with_extension("toml.tmp").exists(),
            "成功路径不应残留临时文件（残留会让下次 rename 撞上半个文件）"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 写入失败只返回 `Err`，不得 panic（调用方靠它记日志而非炸掉写线程）。
    #[test]
    fn write_config_atomically_failure_is_reported_not_panicked() {
        let bogus = b11_temp_dir().join("no-such-subdir").join("config.toml");
        assert!(
            write_config_atomically("x", &bogus).is_err(),
            "父目录不存在时必须返回 Err"
        );
    }

    /// 落盘队列的两条契约（B11）：
    /// ① **入队数 == 处理数**（不丢任务，含「队列满 ⇒ 同步兜底」那一支；
    ///    被判为过期而跳过的也算处理过，见 `PERSIST_DONE` 的说明）；
    /// ② `flush_persist()` 之后内容真的在盘上。
    ///
    /// 连发 200 条（队列容量 64）必然触发若干次 `Full` 兜底分支。
    /// 注：会写到测试二进制同目录的 `config.toml`（`target/debug/deps/`，已 gitignore），
    /// 与 B5 的日志队列用例同构。
    #[test]
    fn persist_queue_contract_never_loses_a_job() {
        let marker = format!("# B11 队列契约 {}\n", std::process::id());
        for i in 0..200 {
            enqueue_persist(format!("{marker}{i}\n"));
        }
        flush_persist();

        assert_eq!(
            PERSIST_DONE.load(Ordering::SeqCst),
            PERSIST_QUEUED.load(Ordering::SeqCst),
            "入队数必须等于处理数（含同步兜底与判为过期而跳过的），否则就是丢任务"
        );
        let on_disk = std::fs::read_to_string(config_path()).expect("落盘后应能读到配置文件");
        assert!(
            on_disk.contains(&marker),
            "盘上内容应来自本用例：{on_disk:?}"
        );
    }
}

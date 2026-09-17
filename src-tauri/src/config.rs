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

pub fn init_config() {
    CONFIG.set(Mutex::new(Config::default())).ok();
    let config = {
        let path = config_path();
        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str(&content) {
                Ok(config) => config,
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

    /// P1-7 回归：**单个字段值非法时不得让整份配置失效**（与字段缺失区分开）。
    ///
    /// 目前 `log_retention` 的 `Deserialize` 对未知值直接 `Err`，而 `#[serde(default)]`
    /// 只在字段缺失时生效 ⇒ 一个非法值仍会打掉整份配置。本测试把该现状钉住，
    /// 作为 P3-9「改枚举 / 集中校验」的基线：**修好后这条断言要翻转**。
    #[test]
    fn invalid_enum_value_currently_kills_whole_config_baseline() {
        let text = "log_retention = \"not_a_real_value\"\n";
        let parsed: Result<super::Config, _> = toml::from_str(text);
        assert!(
            parsed.is_err(),
            "现状（P3-9 未做）：单个枚举字段取值非法会让整份 Config 解析失败。\
             若此断言失败，说明已改为「非法值降级 + 告警」，请同步更新本测试与 P3-9 状态"
        );
    }
}

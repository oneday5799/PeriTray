use crate::dedup::core_name;
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

/// 任务栏信息窗的**内容缩放档位**（用户 2026-09-25 新增设置）。
///
/// ⛔ **作用域边界（用户明确要求，实现时不得越界）**：本档位**只改内容**——
///   图标边长 / 信息文字字号 / 随内容一起缩放的间距与项宽上限；
///   **底衬（窗口高度、圆角）仍按系统 DPI 缩放，不随本项改变**。
///   ⇒ 125% 系统缩放下选 `Default`：图标 32px、字号 11px，而底衬仍是 50px 高 / 圆角 8。
///
/// ⚠️ 落点在 `taskbar_widget::Metrics` 的 `content_dpi`（见该结构文档）；
///   底衬量（`h` / `radius`）恒走 `Metrics::dpi`，两者**分开**换算。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskbarContentScale {
    /// **默认档**：内容按**系统（任务栏）DPI** 布局，即与底衬同口径。
    ///
    /// ⛔ 这一档在 2026-09-29 之前叫 `follow_system`（文案「跟随系统」）——
    ///   改名时**语义不变**，只是文案贴合实际（它本来就是本设置引入前的行为）。
    #[default]
    Default,
    /// **偏小档**：内容按**比系统低一档**的 DPI 布局（125% ⇒ 100%，
    /// 150% ⇒ 125%……），已在最小档则保持 100% 不再降。
    Smaller,
}

impl Serialize for TaskbarContentScale {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Default => serializer.serialize_str("default"),
            Self::Smaller => serializer.serialize_str("smaller"),
        }
    }
}

impl<'de> Deserialize<'de> for TaskbarContentScale {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "default" => Ok(Self::Default),
            "smaller" | "small" => Ok(Self::Smaller),
            // 旧值 `follow_system` = 今天的 `default`（**语义没变，只是改名**）⇒
            // 直接接受，避免升级后用户被静默改档。
            "follow_system" | "followsystem" => Ok(Self::Default),
            // 未知取值降级为默认，而不是让整份 Config 反序列化失败 —— 理由与
            // `LogRetention` 完全同源：`#[serde(default)]` 只在**字段缺失**时生效，
            // 返回 `Err` 会让 `init_config` 回退 `Config::default()`，
            // 用户全部个性化配置被一次性抹掉（P1-7 那条不可逆路径）。
            _ => Ok(Self::default()),
        }
    }
}

/// 任务栏组件当前显示**哪一块内容**（用户选择，重启后保留）。
///
/// ⚠️ 本字段记的是**用户的选择**，**不是**「实际显示什么」：音乐面板在没有媒体会话时
///   不可用，此时会**回落**到设备面板（见 [`taskbar_panel_for`]）。
///   ⇒ 这两件事必须分开：若把它们合成一个字段，「无会话时该显示什么」就没地方表达了。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TaskbarPanel {
    /// 设备信息组件（电量 / 音量 / 悬停提示 / 滚轮调音量）。
    #[default]
    Devices,
    /// 音乐控制组件（封面 + 上一首 / 播放暂停 / 下一首，SMTC）。
    Music,
}

impl Serialize for TaskbarPanel {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Devices => serializer.serialize_str("devices"),
            Self::Music => serializer.serialize_str("music"),
        }
    }
}

impl<'de> Deserialize<'de> for TaskbarPanel {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        match s.to_lowercase().as_str() {
            "music" => Ok(Self::Music),
            _ => Ok(Self::default()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceShortcut {
    pub name: String,
    pub shortcut: Option<String>,
}

/// 任务栏信息窗固定显示的一台**物理设备**。
///
/// 与 `tray_devices`（按名称、控制托盘图标内容）**语义不同**：这里按物理设备身份键
/// 存储，才能区分同型号多实例（两个同款 2.4G 接收器、两只同款耳机）——
/// 按名称存会把它们混成一个。
///
/// 三级字段是**降级匹配链**，不是冗余：
///   · `key`      —— `device_identity::DeviceKey::encode()` 的结果（`c:` 容器 / `i:` 实例 / `n:` 名称）
///   · `fallback` —— key 失效时的兜底（换机、重装驱动会改容器；重装系统会改实例路径）
///   · `alias`    —— 用户自定义显示名，随 key 一起存，避免改名后失联
///
/// ⚠️ **不要存 MAC**：既是隐私，也因为 BTHENUM 实例路径里 MAC 的位置很脆。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PinnedDevice {
    pub key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fallback: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
}

/// 单个固定项是否命中某物理设备。
///
/// 两级匹配，**语义不同**：
///   · `p.key == key` —— 精确身份命中（同一个容器/实例）；
///   · `p.fallback == fallback` —— **同一设备在另一种身份形态下的键**兜底。
///     典型用法：`key` 存容器键（精确但不耐换机/重装驱动），`fallback` 存名称键
///     （`n:<名字>`，模糊但稳定）。容器变了、名字没变时仍能认出是同一台设备。
///
/// 单独拆出**单项**判据，是因为「固定 ⇒ 强制显示」需要**反向**逐项检查
/// 「这一项有没有对应的实际设备」（见 `device_identity::group_taskbar_devices` 末尾的
/// 补建循环）；列表级的 `any` 回答不了「哪一项没被满足」。
/// 两处共用本函数，避免精确/兜底两条规则在两处实现分叉。
pub fn pinned_device_matches(p: &PinnedDevice, key: &str, fallback: Option<&str>) -> bool {
    p.key == key
        || match (p.fallback.as_deref(), fallback) {
            (Some(a), Some(b)) => a == b,
            _ => false,
        }
}

/// 判定某物理设备是否被固定（列表级：**任一**固定项命中即算）。
///
/// 抽成自由函数而非 `Config` 方法，是为了让调用方先取一次快照
/// （`config::with_config(|c| c.pinned_taskbar_devices.clone())`）再逐台设备比对，
/// 避免每台设备各取一次配置锁。
/// 判定某物理设备是否被固定（列表级：**任一**固定项命中即算）。
///
/// 抽成自由函数而非 `Config` 方法，是为了让调用方先取一次快照
/// （`config::with_config(|c| c.pinned_taskbar_devices.clone())`）再逐台设备比对，
/// 避免每台设备各取一次配置锁。
pub fn matches_pinned_taskbar(pinned: &[PinnedDevice], key: &str, fallback: Option<&str>) -> bool {
    pinned
        .iter()
        .any(|p| pinned_device_matches(p, key, fallback))
}
/// 设备信息组件**此刻是否可用**（配置口径，纯函数，可单测）。
///
/// ⭐ 判据 = **总开关开** 且 **已钉设备非空**，两个条件都要：
/// · 只看开关 ⇒ 开了但一台没钉，会在任务栏上出现一个**空窗**；
/// · 只看列表 ⇒ 没法「关掉但保留设备」，这正是该开关存在的原因。
pub fn taskbar_devices_available(c: &Config) -> bool {
    c.taskbar_widget_enabled && !c.pinned_taskbar_devices.is_empty()
}

/// 任务栏组件**此刻该显示哪一块**（`None` = 整个组件不显示）。
///
/// ⭐ 三态而不是布尔（2026-09-28 引入音乐组件时升的）。
///
/// ⛔⛔ **两个组件开关都关 ⇒ 整个组件不显示**（第一层，2026-09-30 用户报）。
/// ```text
/// 记住=Music ∧ 音乐可用        → Music
/// 记住=Devices ∧ 设备可用      → Devices
/// 记住的那块不可用             → 按「开关开着的那块」回落，两块都不可用 → None
/// 两个组件开关都关             → None（**组件整体不存在**）
/// ```
///
/// ⚠️⚠️ **这一层为什么必须单独存在**：下面 ①② 两层判的都是「**显示哪一块**」，
///   判据是「记住的那块可用吗」—— 而**音乐侧的「可用」只看有没有会话、
///   不看 `taskbar_music_enabled`**（这是 ③ 号判据
///   `remembered_panel_wins_over_switch_when_both_enabled` 的子情形明确要的：
///   音乐开关关着、但用户上次选的就是音乐 ⇒ 仍显示音乐）。
///   ⇒ 于是「两个开关都关 + 记住的是音乐 + 有会话」会**穿过 ① 的
///   `Music if music_available` 分支** ⇒ **组件仍然存在**，
///   而且留下的**恰是本应最后关闭的那一块**（用户 2026-09-30 实测现象）。
///
///   ⚛️ **别把「音乐可用」改成含开关**来消这个 bug：那会推翻上面那条
///   **已钉死的**不变量（两个开关是**两个独立维度**，「记住的选择」才是权威）。
///   正解是补上**「组件存不存在」**这一层——它是**第三个维度**，
///   ①② 只管「存在之后显示哪块」。
///
/// ⚠️⚠️ **判据曾经写反过，症状是「点切换按钮没反应」**（用户 2026-09-28 实测）：
/// ```text
/// if music_available && (music_enabled || panel == Music) { return Music }
/// ```
/// 这一行在**音乐开关开着**时恒为真 ⇒ `taskbar_panel` **从头到尾没被读到**。
/// 点切换把字段写成 `Devices`、日志也照打「切换组件 → Devices」，
/// 而显示层下一帧又判回 `Music` ⇒ **屏幕上纹丝不动，且日志完全正常**。
/// 那正是「点了没反应」最难归因的形态：事件到了、状态改了、日志无异常。
/// ⇒ 教训：**「两个开关」与「当前显示哪块」是两个不同维度**，
///   后者必须由前者 + 记住的选择共同决定，不能让开关单独决定。
///   ⛔ 别把本函数改成**一层**：既有「记住 vs 开关」（2026-09-28）、
///     既有「组件存不存在」（2026-09-30）——两个 bug 都源于**维度被合并**。
pub fn taskbar_panel_for(c: &Config, music_available: bool) -> Option<TaskbarPanel> {
    // ⓪ 组件的**存在**判据：两个组件开关都关 ⇒ 什么都不显示。
    //   这一层**先于**「记住的选择」——不然「记住的是音乐」会把已关闭的
    //   音乐组件**复活**，让用户关不掉它。
    if !c.taskbar_widget_enabled && !c.taskbar_music_enabled {
        return None;
    }
    // ① 记住的选择可用 ⇒ 它说了算（这才是「切换按钮」能生效的前提）
    match c.taskbar_panel {
        TaskbarPanel::Music if music_available => return Some(TaskbarPanel::Music),
        TaskbarPanel::Devices if taskbar_devices_available(c) => {
            return Some(TaskbarPanel::Devices);
        }
        _ => {}
    }
    // ② 记住的那块不可用 ⇒ 回落，但**不改写选择**（回落是显示层的事）
    if music_available && c.taskbar_music_enabled {
        return Some(TaskbarPanel::Music);
    }
    if taskbar_devices_available(c) {
        return Some(TaskbarPanel::Devices);
    }
    None
}

/// ⭐ 升级兼容：老配置**没有** `taskbar_widget_enabled` 键（读出来是 `false`），
/// 但它可能**已经钉了设备**——那正是「升级前窗口可见」的状态。
/// ⇒ 读入归一化时把这种情况翻成 `true`，**一次性**改变配置。
///
/// ⚛️ 判据必须是「该键在**文件里**出现过」，**不能**只看 `== false`：
///   用户**主动**关掉开关后值同样是 `false`，若被无脑翻成 `true`，
///   「关闭」就永远关不掉——那是比「升级后消失」更严重的功能失效。
/// ⚠️ `contains` 是**子串**匹配：正文任意位置出现过该字符串即算「出现过」，
///   偏保守（宁可漏迁移、也不误把「用户关过的」翻回开），方向是安全的。
pub fn migrate_taskbar_switch(text: &str, c: &mut Config) -> bool {
    if text.contains("taskbar_widget_enabled") || c.pinned_taskbar_devices.is_empty() {
        return false; // 用户表过态，或从未用过（保持默认关闭）
    }
    c.taskbar_widget_enabled = true;
    true
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
    /// 任务栏信息窗**固定显示**的设备（与 `tray_devices` 的「托盘图标自选设备」是两件事，
    /// 见 `PinnedDevice` 文档）。空表 = 未固定任何设备。
    #[serde(default)]
    pub pinned_taskbar_devices: Vec<PinnedDevice>,
    /// 任务栏信息窗的**总开关**（用户 2026-09-28 要求，默认关闭）。
    ///
    /// ⛔ **与 `pinned_taskbar_devices` 是两个独立维度，缺一不可**：
    /// · 本字段 = 「要不要**显示**这个窗」；
    /// · `pinned_taskbar_devices` = 「这个窗里**显示谁**」。
    /// 关掉本字段**不碰** `pinned_taskbar_devices` ⇒ 用户重新打开时设备列表原样回来，
    /// 不必重新勾选（这正是「关闭后保留任务栏设备的信息」这条要求的落点）。
    /// ⇒ 旧版把「列表非空」当显示开关的做法**必须退役**：那样「关掉」就等于
    /// 「清空列表」，与用户口径直接冲突。
    ///
    /// ⭐ 用裸 `bool` 而**不是** `Option<bool>`（`Option` 路线已试过并否决）：
    ///   serde 对 `None` **跳过序列化** ⇒ 新装用户配置里永远没这个键
    ///   ⇒ 用户在设置页拨了开关、一保存就丢；而想「始终落盘」又会给老配置
    ///   凭空塞进一个 `false`，反而把「升级前可见」的状态写死成关闭。
    ///   ⇒ 升级兼容改由 [`migrate_taskbar_switch`] 在**读入归一化**时一次性处理。
    ///
    /// ⚠️ 裸 `#[serde(default)]`（= false）本身**不是**问题：老用户加载后为 `false`，
    ///   迁移按「键不存在 且 列表非空」把它翻成 `true` 再落盘。
    #[serde(default)]
    pub taskbar_widget_enabled: bool,
    /// 任务栏信息窗的**横向贴靠位置**：`"left"` / `"center"` / `"right"`。
    ///
    /// ⚠️ 贴靠发生在**避让后的视觉空白槽内**，不是整个任务栏（见 `taskbar_widget`）：
    ///   `center` = 槽内居中，不是屏幕居中。
    ///
    /// ⚠️ 具名 helper：真实默认是 `center`，裸 `#[serde(default)]` 会得到空串
    ///   （空串不在 `VALID_TASKBAR_POSITIONS` 里 ⇒ 加载时被归一化回 `center`，
    ///   但中间那一瞬的非法值是多余的，故仍用具名默认）。
    #[serde(default = "default_taskbar_position")]
    pub taskbar_position: String,
    /// 任务栏信息窗是否**固定位置**。
    ///
    /// - `true`：位置由 `taskbar_position` 决定，每次刷新按避让规则重算（不会被压住）；
    /// - `false`：位置由用户**手动拖拽**决定（`taskbar_custom_x`），不再重算
    ///   —— 即「我自己摆，别动它」。
    #[serde(default = "default_true")]
    pub taskbar_position_locked: bool,
    /// 用户**手动拖拽**后放下的窗口左端（相对任务栏客户区的**物理像素**）。
    ///
    /// - `Some(x)`：用户拖过 ⇒ 「不固定位置」时应回到这个位置；
    /// - `None`：用户从没拖过 ⇒ 「不固定」沿用上次绘制的位置。
    ///
    /// ⚠️ 只在 `taskbar_position_locked == false` 时生效。固定位置时**忽略但不清除**
    ///   —— 用户把开关再关掉，就能回到自己放下的地方。
    /// ⚠️ 存的是物理像素，换分辨率/改缩放后可能与预期位置有偏差（读取时会按任务栏
    ///   宽度**钳制**，不会跑出任务栏）。
    #[serde(default)]
    pub taskbar_custom_x: Option<i32>,
    /// 任务栏信息窗的**内容缩放档位**（`"default"` / `"smaller"`）。
    ///
    /// ⛔ 作用域（用户 2026-09-25 明确要求）：**只改内容**（图标 / 文字 / 随内容缩放的
    ///   间距与项宽上限），**底衬仍按系统 DPI 缩放**（窗口高度、圆角不受本项影响）。
    ///   落地见 `taskbar_widget::Metrics` 的 `content_dpi`。
    /// ⚠️ 是 enum 而非 `String`：取值集合固定且只有两档，用 `String + VALID_*` 归一化
    ///   反而多一处可能漂移的清单（`taskbar_position` 那套是历史写法）。
    #[serde(default)]
    pub taskbar_content_scale: TaskbarContentScale,
    /// 「显示音乐控制组件」开关（用户 2026-09-28 新增，**默认关闭**）。
    ///
    /// ⛔ 与 [`Config::taskbar_widget_enabled`] **是两个独立开关**：
    ///   设备信息组件的开关**不控制**音乐组件，反之亦然。
    ///   两者都开时，组件最右侧出现「切换」按钮（见 `taskbar_widget`）。
    #[serde(default)]
    pub taskbar_music_enabled: bool,
    /// 任务栏组件当前显示哪一块内容（用户选择，重启后保留）。
    ///
    /// ⚠️ 记的是**选择**而非「实际显示什么」：音乐面板无会话时不可用，会回落设备面板，
    ///   回落**不改写**本字段（否则一次「临时没播歌」就把用户的选择抹了）。
    #[serde(default)]
    pub taskbar_panel: TaskbarPanel,
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
/// `taskbar_position` 的 serde 默认值（复用 `Config::default()` 的口径，单一来源）
fn default_taskbar_position() -> String {
    "center".to_string()
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
const VALID_TASKBAR_POSITIONS: &[&str] = &["left", "center", "right"];

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

/// 把 `device_names` 里「带括号原串」的条目**归并**一条 `core_name` 短名键（原地、纯内存）。
///
/// **为什么需要它**（方案 D 的读取侧前提）：
/// `rename_device` 从今以后**同时**写「原名」与「`core_name` 短名」两条键（归并双写），
/// 因此新产生的改名天然三处一致。但**历史上**只写过「原名」那一条 ——
/// 那些条目在设备页 / 任务栏（按短名查）**查不到**，用户会看到「改了名的地方没变」。
///
/// **为什么放在这里**：`normalize_config` 是**纯函数**（不读全局状态、不持锁、不做 I/O），
/// 且被**加载路径**（`parse_config_text`）与**写入路径**（`finalize_before_persist`）共用 ⇒
/// 加载时生效、写入时不回潮。放在 `rename_device` 里只能覆盖「新建改名」，历史条目永远补不上；
/// 放在 `load_config` 里会绕开写入路径（下一次 `update_config` 可能把它写没）。
///
/// **不覆盖已存在的短名键**（先到者为准）：避免归并顺序引入不确定性 ——
/// 若原串与短名各自有值，说明用户对「两个不同名字」分别改过名，此时**不猜**，保留既有的。
///
/// 返回 `true` 表示至少插入了一条键。
fn backfill_device_name_keys(config: &mut Config) -> bool {
    // 先收集再插入：`core_name` 的产出可能与某个**已有键**相同（原串本身就是短名形态），
    // 边遍历边插入会让 `HashMap` 的迭代顺序影响结果。
    let mut additions: Vec<(String, String)> = Vec::new();
    for (raw, custom) in &config.device_names {
        let short = core_name(raw);
        if short.trim().is_empty() || short == *raw {
            continue; // 原串本身就是短名（或算不出短名）⇒ 无需归并
        }
        if config.device_names.contains_key(&short) {
            continue; // 短名键已存在 ⇒ 不覆盖
        }
        additions.push((short, custom.clone()));
    }
    if additions.is_empty() {
        return false;
    }
    for (k, v) in additions {
        config.device_names.insert(k, v);
    }
    true
}

/// 把 `PinnedDevice.alias`（**只有**任务栏 tooltip 认它）折进全局 `device_names`。
///
/// ⛔ **为什么需要它**：`resolved_display_name` 的第 1 级是「固定项自带的 alias」，
///   其余所有表面（设备信息页、音量页、托盘、设置页）都只认 `device_names`
///   ⇒ 旧配置里带 alias 的固定项会出现**「任务栏一个名、别处另一个名」**。
///   `alias` 字段如今**没有任何 UI 入口**（旧选择器已退役，只剩
///   `try_toggle_pinned_taskbar_device` 的形参），所以它是**纯历史数据**——
///   折进 `device_names` 既保住用户当初起的名，又让两侧口径一致。
///
/// **键取 `fallback` 的短名部分**（`n:<core_name>`）——与 `resolved_display_name`
/// 构造 `fallback` 的算法是同一个（`DeviceKey::Name(core_name(name))`），
/// 写出来的键因此正是各表面查找时算出的那个短名。
///
/// **不覆盖已存在的 `device_names` 条目**（先到者为准）：用户后来在别处改过名就以那个为准。
/// 返回 `true` 表示至少写入了一条。
fn fold_pinned_alias_into_device_names(config: &mut Config) -> bool {
    // 先收集再改：`device_names` 的插入与 `alias` 的清空都会改变被遍历的结构。
    //
    // ⭐⭐ **折叠后必须清掉 `alias` 本身**（P0，用户 2026-09-28 评审定案）：
    //   `resolved_display_name` 的第 1 级是 `pin.alias`、**优先于** `device_names`。
    //   若只折进全局表却留着 alias，用户日后全局改名为 Y 时：
    //   任务栏仍显示 alias「X」、其余各处显示 Y ⇒ **刚修掉的不一致原样复发**，
    //   而且只在旧配置（带 alias）上出现，极难联想到是这里。
    //   ⇒ 这一级历史层级就此退休：值已进全局表，别名不再有独立含义。
    //
    // ⚠️ `device_names` 已有同键条目时**不覆盖**（用户后来在别处改过名，以那个为准），
    //   但 alias **照样清**——否则又变成「任务栏一个名、别处另一个名」。
    //
    // ⚠️ 算不出短名（既无 `n:` 兜底键、`key` 也不是 `n:` 形态）时**保留 alias**：
    //   宁可留着不一致，也不能把用户当初起的名丢掉。
    let mut additions: Vec<(String, String)> = Vec::new();
    let mut to_clear: Vec<usize> = Vec::new();
    for (i, p) in config.pinned_taskbar_devices.iter().enumerate() {
        let Some(alias) = p.alias.as_deref().map(str::trim).filter(|a| !a.is_empty()) else {
            continue;
        };
        // `fallback` 本身就是 `n:<短名>`；退化时从 `key` 的 `n:` 形态再取一次。
        // ⚠️ 一律取**owned** `String`：`core_name` 返回 String，与 `&str` 混用会
        //    逼出借用技巧（曾写成 `Box::leak` ⇒ 直接内存泄漏，禁）。
        let short: Option<String> = p
            .fallback
            .as_deref()
            .and_then(|f| f.strip_prefix("n:"))
            .map(str::to_string)
            .or_else(|| p.key.strip_prefix("n:").map(core_name));
        let Some(short) = short
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
        else {
            continue;
        };
        if !config.device_names.contains_key(&short) {
            additions.push((short, alias.to_string()));
        }
        to_clear.push(i);
    }
    if additions.is_empty() && to_clear.is_empty() {
        return false;
    }
    for (k, v) in additions {
        config.device_names.insert(k, v);
    }
    for i in to_clear {
        config.pinned_taskbar_devices[i].alias = None;
    }
    true
}

/// 应用一次设备改名：**归并写入 / 归并删除**（方案 D 的写入侧，纯函数，便于单测）。
///
/// `original` 是前端传来的**名字**（音量页 = 带括号原串 `扬声器 (DUNU DTC100pro)`；
/// 设备页 = `core_name` 短名 `DUNU DTC100pro`），**不带身份键** ——
/// 改名对话框只传名字（`common.js:421 showRenameDialog`），后端无从得知它属于哪台设备。
///
/// ⇒ 于是「同一台设备在两个页面名字形态不同」这件事，只能靠**把两种形态都写上**来抹平：
/// 读取侧 `resolve_device_name` 三级回退，两处都能命中。
///
/// ⛔ **若只写 `device_names[original]`**（旧实现），从音量页改名后设备页/任务栏
/// **仍显示原名** —— 既不报错也不告警的**静默失效**（已由单测钉住）。
///
/// 语义与旧实现完全兼容：
/// · `new_name` 为空、或与原名相同 ⇒ 视为「恢复默认」，**两种形态都删净**；
/// · 否则两种形态都写入同一个自定义名。
///
/// ⚠️ **为什么抽成独立纯函数**：`rename_device` 是 `#[tauri::command]`、需要 `AppHandle`
/// 才能 `emit`，直接单测代价高；而这段归并逻辑恰恰**必须**被单测钉住（它是静默失效的来源）。
/// 抽成纯函数后既可直接测，也与本仓 `normalize_config` 的既有风格一致。
pub fn apply_device_rename(config: &mut Config, original: &str, new_name: &str) -> bool {
    // ⭐ **后端也要 trim**（P4）：前端 `showRenameDialog` 虽有 `input.value.trim()`，
    //   但命令是公开入口，直接调用可存进 `"  "` ⇒ 各表面的 `if (custom)` 判空
    //   失败、静默回落原名，表现为「改名没生效」且无处可查。
    let new_name = new_name.trim();
    let short = core_name(original);
    if new_name.is_empty() || new_name == original {
        // 恢复默认：**删净所有指向同一台设备的键**，不能只删入口传来的那一个形态。
        //
        // ⛔ 这里有个非对称陷阱：**写入**是「一对多」（原名 + 短名都写），
        //   而**删除**若只知道入口形态，就会留下另一个形态的键 ⇒
        //   用户从设备页（短名入口）点「恢复默认」后，音量页那条键还在 ⇒
        //   **名字没变回去**（静默、且只在一半的页面里可见）。
        //   ⇒ 判据必须与归并写入**同一套**：删掉 `original` 本身，
        //     以及所有 `core_name(k) == core_name(original)` 的键（即同一台设备的各形态）。
        let target = short.clone();
        let before = config.device_names.len();
        config
            .device_names
            .retain(|k, _| k != original && core_name(k) != target);
        return config.device_names.len() != before;
    }
    // ⭐ 归并集 = {入口键, 短名键} ∪ {**已存在的**同 `core_name` 键}（C）。
    //
    // ⛔ 为什么必须扫已存在的键：只写前两条时，若配置里还留着**另一种**长形态的
    //   键（历史条目、或同一台设备有两个不同端点名如 `扬声器 (X)` 与 `耳机 (X)`），
    //   它会保持旧值 ⇒ 配置里长期存在**互相矛盾的两条别名**。显示上暂时无害
    //   （读取永远短名键优先，加载时 `backfill_device_name_keys` 也保证短名键存在），
    //   但这依赖一条**隐式不变量**；哪天有人调整读取顺序，它立刻变成显示 bug。
    //   ⇒ 与删除侧（按 `core_name` 全删）保持对称：写也按 `core_name` 全写。
    //
    // ⚠️ 先收集再插入：`HashMap` 迭代期间不得写入。
    let mut keys: Vec<String> = Vec::new();
    for k in [original.to_string(), short.clone()] {
        if !keys.contains(&k) {
            keys.push(k);
        }
    }
    for k in config.device_names.keys() {
        if !keys.contains(k) && core_name(k) == short {
            keys.push(k.clone());
        }
    }
    let mut changed = false;
    for k in keys {
        if config.device_names.get(&k).map(String::as_str) != Some(new_name) {
            config.device_names.insert(k, new_name.to_string());
            changed = true;
        }
    }
    changed
}

/// 显示名长度上限（**只管显示，不管数据**）。
///
/// ⭐ 为什么需要：任务栏窗口是**原生分层窗**，宽度由文字估算撑开（`current_content()`
///   的估宽按「宁可高估、不可低估」），一个几百字的别名会把窗口撑到超出屏幕。
///   而四个网页表面靠 CSS `text-overflow: ellipsis` 天然截断，不需要这个上限。
/// ⚠️ **只加在渲染前的最后一步**（`resolved_display_name` / `resolve_audio_display_name`），
///   **不能**加在 `resolve_device_name` 上——重命名对话框的输入框预填也走那条路，
///   一旦截断，用户编辑长名字时会看到被削过的初值，保存就把别名毁了。
///   配置文件里始终保留用户原意。
pub const MAX_DISPLAY_NAME_CHARS: usize = 32;

/// 截断超长显示名，尾部补省略号（按**字符**计，不按字节——中文名按字节会砍掉一半汉字）。
pub fn clamp_display_name(name: &str) -> String {
    if name.chars().count() <= MAX_DISPLAY_NAME_CHARS {
        return name.to_string();
    }
    let mut out: String = name.chars().take(MAX_DISPLAY_NAME_CHARS - 1).collect();
    out.push('…');
    out
}

/// 按「短名优先、原名回退」解析设备展示名（方案 D 的读取侧，三级回退）。
///
/// ```
/// 1) device_names[core_name(raw_name)]   → 命中即用   ← 音量页与设备页靠这条统一
/// 2) device_names[raw_name]              → 回退（历史条目 / 原串本身即短名）
/// 3) 任一 `core_name` 相同的键（键名排序取首个）← 防「一个形态空白、另一个有效」
/// 4) raw_name                            → 原名
/// ```
///
/// ⛔ 各级都必须跳过**空白**值：JS 侧 `if (custom)` 把 `""` 判假并回落原名，
///   Rust 侧若照收就会在任务栏/托盘/通知里显示空白 ⇒ 两侧口径必须一致。
/// ⚠️ 本函数与前端 `common.js` 的 `lookupDeviceAlias` 是**同一个判据的两份实现**，
///   改一侧必须同步另一侧，否则又变成「同一个设备两套名字」。
///
/// ⚠️ **为什么必须有第 1 级**：音量页用的 `AudioDevice.name` 是**未归一化原串**
/// （`扬声器 (DUNU DTC100pro)`），而设备页用的 `Device.name` 是 `core_name` **短名**
/// （`DUNU DTC100pro`）。改名入口**只传名字字符串、不传身份键**
/// （`common.js:421 showRenameDialog`）⇒ 后端无法知道这个名字属于哪台设备。
/// 归并双写 + 三级回退是**唯一能零改前端**把两侧统一起来的路径。
///
/// ⛔ **不要用 `Device.name` 反推身份**（见 `device_identity.rs` 同名纪律）——
/// 本函数只做**显示名解析**，不参与身份判定。
pub fn resolve_device_name(raw_name: &str, config: &Config) -> String {
    resolve_device_name_in(raw_name, &config.device_names)
}

/// 同 [`resolve_device_name`]，但只吃**改名表本身**。
///
/// ⭐ 存在的理由：`battery_notify` 为避免在通知循环里持配置锁，只克隆了
/// `device_names` 这一张表（`Config` 其余字段都很沉）。**判据必须与上面那个
/// 函数逐字一致**（AGENTS.md「复用同一判据 = 同一函数」），故这里只做转发，
/// 不复制 `core_name` 查找逻辑。
pub fn resolve_device_name_in(
    raw_name: &str,
    device_names: &std::collections::HashMap<String, String>,
) -> String {
    // ⚠️⚠️ **空白值必须当「无别名」**（A）：`get()` 命中空串会 `return ""`，
    //   而 JS 侧 `formatDeviceName` 的 `if (custom)` 把 `""` 判假、回落原名
    //   ⇒ 同一个设备在**任务栏 tooltip / 托盘菜单 / 低电量通知**显示空白，
    //   四个页面却正常。只有手改配置才可能写进空值，但「两处实现同一判据」
    //   的老毛病就是这样一点点长出来的。
    if let Some(v) = device_names
        .get(&core_name(raw_name))
        .filter(|v| !v.trim().is_empty())
    {
        return v.clone();
    }
    if let Some(v) = device_names.get(raw_name).filter(|v| !v.trim().is_empty()) {
        return v.clone();
    }
    // ⭐ 第 4 级：**任一形态**的同源键有有效别名就用它。
    //
    // ⛔ 为什么必须有：前两级只看 `core_name(raw)` 与 `raw` 两个**确定的**键。
    //   当「短名键是空白、长形态键有值」时（手改配置 / 历史条目），
    //   音量页走长形态能取到别名、设备页走短名却只能回落原名
    //   ⇒ **同一设备两套名字**，正是本轮一直在消灭的那类不一致。
    //
    // ⚠️ 多个同源键同时有效时按**键名排序**取第一个：`HashMap` 迭代顺序不确定，
    //   不排序就会「同一个配置每次启动显示的名字可能不同」。
    if let Some(v) = device_names
        .iter()
        .filter(|(k, v)| core_name(k) == core_name(raw_name) && !v.trim().is_empty())
        .min_by_key(|(k, _)| k.as_str())
        .map(|(_, v)| v.clone())
    {
        return v;
    }
    raw_name.to_string()
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
    changed |= normalize_choice(
        &mut config.taskbar_position,
        VALID_TASKBAR_POSITIONS,
        "center",
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

    // ⭐ 历史改名回填（方案 D）：给「带括号原串」条目补一条 `core_name` 短名键。
    // 纯内存归并 ⇒ 符合本函数的「不持锁、不做 I/O」契约。
    changed |= backfill_device_name_keys(config);

    // ⭐ 固定项自带别名折进全局改名表：让「只有任务栏认 alias」的历史数据
    // 对所有表面可见（详见该函数注释）。放在回填之后 ⇒ alias 不会抢已有短名键。
    changed |= fold_pinned_alias_into_device_names(config);

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
            pinned_taskbar_devices: vec![],
            taskbar_widget_enabled: false,
            taskbar_position: default_taskbar_position(),
            taskbar_position_locked: true,
            taskbar_custom_x: None,
            taskbar_content_scale: TaskbarContentScale::default(),
            // 新增两字段：音乐开关默认**关**、面板默认设备（与 serde default 一致）
            taskbar_music_enabled: false,
            taskbar_panel: TaskbarPanel::default(),
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

/// 配置文件路径（`<可写根目录>/config.toml`）。
///
/// 根目录走 [`crate::process::writable_root`] 而非 `exe_dir()`：**MSIX 的包安装目录只读**，
/// 原先写在这里的每一次保存都会失败（且失败提示本身也写不进日志，全静默）。
/// 非 MSIX 环境下 `writable_root()` 与 `exe_dir()` 同值，故老用户路径不变。
fn config_path() -> std::path::PathBuf {
    crate::process::writable_root().join("config.toml")
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
    let mut normalized = normalize_config(&mut config);
    // ⭐ 升级兼容走这里：老配置没有 `taskbar_widget_enabled` 键，
    //   但可能已钉了设备 ⇒ 那正是「升级前窗口可见」的状态，翻成 `true`。
    //   ⚠️ 必须在 `normalize_config` **之后**跑：它才刚把非法值修干净，
    //   此时 `pinned_taskbar_devices` 才是可信的最终值。
    normalized |= migrate_taskbar_switch(text, &mut config);
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
    // ⭐ 任务栏开关的升级迁移**必须立刻落盘**（与上面的「归一化不写盘」相反）。
    //
    // ⚠️ 为什么这一条要破例：`migrate_taskbar_switch` 是**一次性**改写
    // （老配置 ⇒ 补出 `taskbar_widget_enabled = true`）。不落盘的后果是
    // **每次启动都重跑一遍**，而期间用户只要在设置页把开关关掉，
    // 下一启动又会被翻回 true ⇒ 「**关不掉**」——一个不可自行恢复的功能性失效。
    // 归一化不落盘是安全的（幂等、可反复重算），迁移不落盘则不是。
    {
        let already_on_disk = std::fs::read_to_string(config_path())
            .map(|t| t.contains("taskbar_widget_enabled"))
            .unwrap_or(true); // 读不到文件 ⇒ 交给正常写盘流程，不在此处干预
        let need_persist = crate::state::lock_unpoisoned(CONFIG.get().unwrap())
            .taskbar_widget_enabled
            && !already_on_disk;
        if need_persist {
            // ⛔ 走 `write_config_atomically`（**同步**）而**不是** `enqueue_persist`：
            //   后者要调用方先把配置**序列化好**传进来，而在 `init_config` 这个位置
            //   持锁读配置再序列化会与写盘线程抢锁；且迁移本就该「启动时一次定局」。
            let snapshot = crate::state::lock_unpoisoned(CONFIG.get().unwrap()).clone();
            match toml::to_string_pretty(&snapshot) {
                Ok(text) => {
                    if write_config_atomically(&text, &config_path()).is_ok() {
                        standard_log!(
                            "[config] 任务栏开关迁移：老配置已含已钉设备 ⇒ 置为开启并写盘（仅此一次）"
                        );
                    }
                }
                Err(e) => standard_log!("[config] 任务栏开关迁移：序列化失败 err={}", e),
            }
        }
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
            pinned_taskbar_devices,
            taskbar_widget_enabled,
            taskbar_position,
            taskbar_position_locked,
            taskbar_custom_x,
            taskbar_content_scale,
            taskbar_music_enabled,
            taskbar_panel,
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
        apply_device_rename, claim_revision, clamp_display_name, config_lock_held, config_path,
        default_battery_refresh_secs, default_battery_thresholds, enqueue_persist,
        finalize_before_persist, flush_persist, fold_pinned_alias_into_device_names, merge_config,
        migrate_taskbar_switch, normalize_config, parse_config_text, resolve_device_name,
        resolve_device_name_in, revision_is_latest, taskbar_devices_available, with_config,
        write_config_atomically, Config, PinnedDevice, MAX_DISPLAY_NAME_CHARS, MERGED_FIELD_NAMES,
        PERSIST_DONE, PERSIST_QUEUED, VALID_TASKBAR_POSITIONS,
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
            // ⭐ 任务栏贴靠位置：只认 left/center/right 三值（前端下拉也仅这三项）。
            //    非法值若不归一化，后端会把它当作 `_` 分支（退化成居中）而**前端下拉
            //    找不到对应项 ⇒ 按钮文案停在旧值**，界面上看着「选好了」实际是错的。
            taskbar_position: "middle".to_string(),
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
        assert_eq!(cfg.taskbar_position, "center");
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
            taskbar_position: "right".to_string(),
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

    /// `taskbar_position` 的三个合法值**逐个**钉住。
    ///
    /// ⭐ 为什么单列一条：`normalize_choice` 的 fallback 恰好就是 `"center"`，
    ///   所以 `VALID_TASKBAR_POSITIONS` 里把 `"center"` 拼错**测不出来**（错值也会
    ///   被改回 center）；但把 `"left"` / `"right"` 拼错就会**静默退化**成居中 ——
    ///   界面照常显示、不报错，只是「靠左/靠右」永远无效。本条用 `assert!(!changed)`
    ///   把「合法值不被改写」钉住，拼错即转红。
    #[test]
    fn taskbar_position_accepts_all_three_values_verbatim() {
        for value in ["left", "center", "right"] {
            let mut cfg = Config {
                taskbar_position: value.to_string(),
                ..Default::default()
            };
            assert!(
                !normalize_config(&mut cfg),
                "`{value}` 是合法贴靠位置，不应被归一化改写"
            );
            assert_eq!(cfg.taskbar_position, value, "合法值必须逐字保留");
        }
        // 与前端下拉 `data-value` 一一对应（`settings.html` 的 win-combo-item）——
        // 少一个值会让某一档永远选不出来，且没有任何报错。
        assert_eq!(
            VALID_TASKBAR_POSITIONS,
            &["left", "center", "right"],
            "合法值集合必须与设置页下拉的三项逐字一致"
        );
    }

    /// `taskbar_content_scale` 的两个合法值**逐个**钉住（逐字保留）。
    ///
    /// ⭐ 为什么单列一条：这两个字面量必须与设置页下拉的 `data-value`
    ///   （`settings.html` 的 `win-combo-item`）**逐字一致** —— 任一侧拼错都会让
    ///   该档永远选不出来（后端降级回 `default`、前端找不到对应项而停在旧文案），
    ///   且不报错、不 panic。
    #[test]
    fn taskbar_content_scale_accepts_both_values_verbatim() {
        for (raw, want) in [
            ("default", super::TaskbarContentScale::Default),
            ("smaller", super::TaskbarContentScale::Smaller),
        ] {
            let cfg: super::Config = toml::from_str(&format!("taskbar_content_scale = \"{raw}\""))
                .expect("合法档位必须能解析");
            assert_eq!(cfg.taskbar_content_scale, want, "合法值必须逐字保留");
        }
    }

    /// ⛔ 未知取值必须**降级为默认**，且不得牵连同一份文件里的其它字段
    /// （返回 `Err` ⇒ 整份配置回退 `Config::default()` ⇒ 用户全部配置被抹掉，P1-7）。
    #[test]
    fn taskbar_content_scale_unknown_falls_back_to_default() {
        let text = "auto_start = true\n\
                    taskbar_content_scale = \"not_a_real_value\"\n";
        let cfg: super::Config = toml::from_str(text).expect("未知档位不得让整份 Config 解析失败");
        assert_eq!(
            cfg.taskbar_content_scale,
            super::TaskbarContentScale::Default,
            "未知档位应降级为默认档"
        );
        assert!(cfg.auto_start, "非法档位不得牵连其它字段");
    }

    /// 默认档必须是「不跟随系统缩放」（用户指定），且**缺键时也取它**。
    ///
    /// ⭐ 两条断言缺一不可：`Default` 的 derive 实现与 `Deserialize` 的缺键路径是
    ///   **两处独立代码**，只测一处的话另一处反转（`#[default]` 标错变体、
    ///   或把 `#[serde(default)]` 换成具名 helper）测不出来。
    #[test]
    fn taskbar_content_scale_defaults_to_not_following_system() {
        assert_eq!(
            super::TaskbarContentScale::default(),
            super::TaskbarContentScale::Default,
            "默认档必须是「默认缩放大小」（不跟随系统缩放）"
        );
        let cfg: super::Config = toml::from_str("").expect("空配置必须能解析");
        assert_eq!(
            cfg.taskbar_content_scale,
            super::TaskbarContentScale::Default,
            "缺键时必须取默认档"
        );
    }

    /// ⭐⭐ **旧档位 `follow_system` 必须被接受，且映射到 `Default`**（2026-09-29 改档）。
    ///
    /// ⛔ 那一档只是**改名**（「跟随系统」→「默认」），语义没变 ⇒ 升级后不能把用户
    ///   静默改档，否则「我明明选的是跟随系统，怎么变了」无从排查。
    #[test]
    fn legacy_follow_system_value_maps_to_default() {
        for raw in ["follow_system", "followsystem", "FOLLOW_SYSTEM"] {
            let cfg: super::Config = toml::from_str(&format!("taskbar_content_scale = \"{raw}\""))
                .expect("旧档位字面量必须仍能解析（不得让整份配置回退）");
            assert_eq!(
                cfg.taskbar_content_scale,
                super::TaskbarContentScale::Default,
                "旧值 {raw} 应映射到 Default（改名不改语义）"
            );
        }
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
    /// **跳过 `None`**，所以 `Config::default()` 里有 7 个 `Option` 字段压根不出现在
    /// 结果里（`legacy_log_enabled` + 5 个 `shortcut_*` + `taskbar_custom_x`）。数个数就得硬编码偏移量，
    /// 而偏移量本身也会漂。
    ///
    /// ⚠️ **新增可选（`Option`）字段时**：请同时在下面的 `probe` 里把它设为 `Some`，
    /// 否则它默认不出现在序列化结果里，本用例覆盖不到它。
    /// （`taskbar_custom_x` 就是这样被本用例抓出来的 —— 见下方 `probe`。）
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
            taskbar_custom_x: Some(120),
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

    // ── taskbar_widget_enabled：升级兼容迁移 ───────────────────────

    fn cfg_with_pins(n: usize, enabled: bool) -> Config {
        Config {
            pinned_taskbar_devices: (0..n)
                .map(|i| PinnedDevice {
                    key: format!("c:{}", i),
                    fallback: None,
                    alias: None,
                })
                .collect(),
            taskbar_widget_enabled: enabled,
            ..Default::default()
        }
    }

    /// ⭐ 老配置（**没有**这个键）+ 已钉设备 ⇒ 迁移成「开」，维持升级前可见性。
    ///
    /// 可证伪：把 `migrate_taskbar_switch` 改成恒 `false` ⇒ 本条转红，
    /// 而真机上表现为「老用户升级后任务栏窗口凭空消失」。
    #[test]
    fn migration_opens_switch_for_legacy_config_with_devices() {
        let legacy = "pinned_taskbar_devices = [{ key = \"c:abc\" }]
";
        let mut c = Config {
            pinned_taskbar_devices: vec![PinnedDevice {
                key: "c:abc".into(),
                fallback: None,
                alias: None,
            }],
            ..Default::default()
        };
        assert!(!c.taskbar_widget_enabled, "前提：读出来默认是关");
        assert!(migrate_taskbar_switch(legacy, &mut c), "老配置应触发迁移");
        assert!(c.taskbar_widget_enabled, "迁移后必须为开，否则窗口凭空消失");
        assert_eq!(
            c.pinned_taskbar_devices.len(),
            1,
            "⭐ 迁移**不得**动设备列表"
        );
    }

    /// ⛔ 用户**主动关过**的开关**不得**被翻回开——否则「关闭」永远关不掉。
    ///
    /// 这是本迁移最危险的失败模式：它是**不可逆的数据改写**，
    /// 一次误判就让用户永久失去关闭能力（只能手改配置文件）。
    #[test]
    fn migration_does_not_override_explicit_user_choice() {
        let explicit_off = "taskbar_widget_enabled = false
pinned_taskbar_devices = [{ key = \"c:abc\" }]
";
        let mut c = cfg_with_pins(1, false);
        assert!(
            !migrate_taskbar_switch(explicit_off, &mut c),
            "键已出现过 ⇒ 用户表过态 ⇒ 不得迁移"
        );
        assert!(!c.taskbar_widget_enabled, "用户的「关」必须被尊重");
    }

    /// 新装用户（从未钉过设备）⇒ 保持默认关闭，**不触发**迁移。
    #[test]
    fn migration_is_noop_for_fresh_install() {
        let fresh = "log_level = \"standard\"
";
        let mut c = Config::default();
        assert!(!migrate_taskbar_switch(fresh, &mut c), "无设备 ⇒ 不迁移");
        assert!(
            !c.taskbar_widget_enabled,
            "新装必须是关闭（用户口径：默认关闭）"
        );
    }

    /// ⭐ 迁移只在**键不存在**时发生；显式 `true` 同样不得被改。
    #[test]
    fn migration_preserves_explicit_true() {
        let txt = "taskbar_widget_enabled = true
";
        let mut c = cfg_with_pins(1, true);
        assert!(!migrate_taskbar_switch(txt, &mut c));
        assert!(c.taskbar_widget_enabled, "显式开必须保持");
    }

    /// 迁移必须**只改开关一个字段**，其余配置逐字不动。
    #[test]
    fn migration_touches_nothing_but_the_switch() {
        let mut c = cfg_with_pins(3, false);
        let before_pins = c.pinned_taskbar_devices.clone();
        let before_names = c.device_names.clone();
        let before_pos = c.taskbar_position.clone();
        migrate_taskbar_switch(
            "pinned_taskbar_devices = []
",
            &mut c,
        );
        assert_eq!(c.pinned_taskbar_devices, before_pins, "设备列表不得被改");
        assert_eq!(c.device_names, before_names, "重命名表不得被改");
        assert_eq!(c.taskbar_position, before_pos, "位置设置不得被改");
    }

    /// ⭐ 端到端：老配置文本经 `parse_config_text` 后，窗口仍应可见。
    #[test]
    fn parse_config_text_migrates_legacy_text() {
        let legacy = "pinned_taskbar_devices = [{ key = \"c:abc\" }]
";
        let (c, changed) = parse_config_text(legacy).expect("老配置应能解析");
        assert!(changed, "应报告发生了迁移");
        assert!(
            taskbar_devices_available(&c),
            "老用户升级后窗口**必须**仍然可见"
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

    // ── 第 2 层（方案 D）：回填 + 三级回退解析 ──────────────────────

    /// ⭐ **T2-5 回填**：植入一条**历史形态**条目（只有带括号原串的键）⇒
    /// 归一化后必须**同时**存在原串键与 `core_name` 短名键，且短名键指向同一个自定义名。
    ///
    /// ⚠️ **本机 `[device_names]` 为空 ⇒ 真机无法验证**（回填在本机是空操作）
    /// ⇒ 只能靠本单测，**不得**声称「真机验证通过」。
    #[test]
    fn normalize_config_backfills_core_name_key_for_legacy_entry() {
        let mut cfg = Config::default();
        // 历史条目：只有「带括号原串」这一条（旧版 `rename_device` 只写这条）
        cfg.device_names
            .insert("扬声器 (DUNU DTC100pro)".to_string(), "我的DAC".to_string());

        assert!(
            normalize_config(&mut cfg),
            "发生回填 ⇒ 必须报告 changed=true"
        );

        assert_eq!(
            cfg.device_names
                .get("扬声器 (DUNU DTC100pro)")
                .map(String::as_str),
            Some("我的DAC"),
            "原串键必须保留（音量页仍按它查）"
        );
        assert_eq!(
            cfg.device_names.get("DUNU DTC100pro").map(String::as_str),
            Some("我的DAC"),
            "短名键必须被回填（设备页/任务栏按它查）"
        );
        assert_eq!(cfg.device_names.len(), 2, "恰好补一条，实际 {cfg:?}");
    }

    /// ⭐ **回填幂等性**：连跑两次 ⇒ 第二次返回 `false`、内容**逐字不变**。
    ///
    /// 幂等是关键 —— `normalize_config` 在**每次写入**（`finalize_before_persist`）都会跑，
    /// 若它每次都「报告 changed」，`merge_config` / 落盘层会误判为「配置有变化」而反复写盘。
    #[test]
    fn backfill_is_idempotent() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("耳机 (小爱音箱-9205)".to_string(), "客厅音箱".to_string());

        assert!(normalize_config(&mut cfg), "首次必须回填");
        let after_first = cfg.device_names.clone();

        assert!(
            !normalize_config(&mut cfg),
            "第二次必须报告『无变化』（否则每次写入都误判为变更）"
        );
        assert_eq!(cfg.device_names, after_first, "内容必须逐字不变");
        assert_eq!(cfg.device_names.len(), 2);
    }

    /// ⛔ **回填不覆盖已存在的短名键**（先到者为准）。
    ///
    /// 场景：用户先后对「带括号原串」与「短名」**分别**改过名（历史数据里可能出现，
    /// 因为两个页面的入口喂的就是不同形态的名字）。此时**不猜哪个对**，保留既有的短名值。
    #[test]
    fn backfill_does_not_overwrite_existing_short_key() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("扬声器 (X)".to_string(), "A".to_string());
        cfg.device_names
            .insert("扬声器 X".to_string(), "B".to_string());

        normalize_config(&mut cfg);

        assert_eq!(
            cfg.device_names.get("扬声器 X").map(String::as_str),
            Some("B"),
            "既有短名键不得被覆盖"
        );
        assert_eq!(
            cfg.device_names.get("扬声器 (X)").map(String::as_str),
            Some("A"),
            "原串键保持原值"
        );
    }

    /// 回填的**反控**：键本身就是短名形态（`core_name` 看不出括号）⇒ 不该多插一条。
    /// 否则每次归一化都会把 `device_names` 撑大（在设备页改名的场景下会翻倍）。
    #[test]
    fn backfill_skips_keys_that_are_already_short() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), "我的DAC".to_string());
        cfg.device_names
            .insert("VID_1234".to_string(), "我的手柄".to_string());

        let changed = normalize_config(&mut cfg);

        assert!(!changed, "短名键无需回填 ⇒ 不得报告变化");
        assert_eq!(cfg.device_names.len(), 2, "不得新增键，实际 {cfg:?}");
    }

    /// ⭐ **T2-2 三级回退**：`resolve_device_name` 的四种情形。
    ///
    /// 这是方案 D 的**读取侧核心** —— 音量页喂带括号原串、设备页喂短名，
    /// 两种形态都必须解析到**同一个**自定义名（否则就是「改名后某处没变」的静默失效）。
    #[test]
    fn resolve_device_name_falls_back_in_three_levels() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), "我的DAC".to_string());
        cfg.device_names
            .insert("耳机 (小爱音箱-9205)".to_string(), "客厅音箱".to_string());

        // 第 1 级：音量页的带括号原串 ⇒ 经 core_name 命中短名键（**DUNU 回归**）
        assert_eq!(
            resolve_device_name("扬声器 (DUNU DTC100pro)", &cfg),
            "我的DAC",
            "音量页的带括号原串必须解析到自定义名"
        );
        // 第 1 级：设备页的短名直接命中
        assert_eq!(resolve_device_name("DUNU DTC100pro", &cfg), "我的DAC");

        // 第 2 级：原串本身就是键（历史条目形态）—— 这里用一条无括号的键验证回退
        assert_eq!(
            resolve_device_name("耳机 (小爱音箱-9205)", &cfg),
            "客厅音箱",
            "原串键直接命中（core_name 得到的短名不同时回落第 2 级）"
        );

        // 第 3 级：都不命中 ⇒ 原样返回（**必须**，否则所有未改名设备的显示名全空）
        assert_eq!(
            resolve_device_name("扬声器 (Steam Streaming Speakers)", &cfg),
            "扬声器 (Steam Streaming Speakers)",
            "未改名的设备必须原样返回"
        );
        assert_eq!(resolve_device_name("", &cfg), "", "空名不得 panic");
    }

    /// ⭐ 短名入口在**只有长形态键**时也必须解析到自定义名。
    ///
    /// ⚠️ 契约已变更（2026-09-28）：本用例原先断言「未回填时设备页**拿不到**名」
    ///   （那是在记录一个待修的静默失效）。现在 `resolve_device_name_in` 多了
    ///   **第 3 级**（任一同 `core_name` 键），所以**回填之前**就已经能解析到了。
    ///   回填仍然有价值：它让「短名键」存在，使绝大多数查询走 O(1) 的前两级、
    ///   不必每次扫全表 —— 但它**不再是「能不能解析到」的前提**。
    #[test]
    fn resolve_device_name_finds_the_alias_from_any_form() {
        let mut cfg = Config::default();
        // 模拟「历史数据」：只有带括号原串那一条（未经归一化的旧配置）
        cfg.device_names
            .insert("扬声器 (DUNU DTC100pro)".to_string(), "我的DAC".to_string());

        // 未回填：短名键不存在，但第 3 级按 `core_name` 找到长形态那条 ⇒ 照样命中
        assert_eq!(
            resolve_device_name("DUNU DTC100pro", &cfg),
            "我的DAC",
            "短名入口必须能从长形态键解析到（否则同一设备两套名字）"
        );

        // 归一化之后：短名键已存在，走 O(1) 的第 1 级，结论一致
        normalize_config(&mut cfg);
        assert!(
            cfg.device_names.contains_key("DUNU DTC100pro"),
            "回填应补出短名键"
        );
        assert_eq!(
            resolve_device_name("DUNU DTC100pro", &cfg),
            "我的DAC",
            "回填后设备页/任务栏必须拿到自定义名"
        );
    }

    // ── T2-3：`apply_device_rename` 归并写入 / 归并删除 ──────────────

    /// ⭐ **从音量页改名（原名 = 带括号原串）⇒ 两种形态都落入配置**。
    ///
    /// ⛔ 这是**必须靠单测**钉住的核心：它的失效方式是「设备页仍显示原名」，
    /// 既不报错也不告警 —— 手测很容易漏（用户只改一处、看起来"生效了"）。
    #[test]
    fn apply_device_rename_from_volume_page_writes_both_forms() {
        let mut cfg = Config::default();

        assert!(
            apply_device_rename(&mut cfg, "扬声器 (DUNU DTC100pro)", "我的DAC"),
            "写入必须报告 changed"
        );

        assert_eq!(
            cfg.device_names
                .get("扬声器 (DUNU DTC100pro)")
                .map(String::as_str),
            Some("我的DAC"),
            "原名键（音量页按它查）"
        );
        assert_eq!(
            cfg.device_names.get("DUNU DTC100pro").map(String::as_str),
            Some("我的DAC"),
            "短名键（设备页/任务栏按它查）—— 缺了它就是静默失效"
        );
        assert_eq!(cfg.device_names.len(), 2);

        // 三处渲染点全部解析到自定义名
        assert_eq!(
            resolve_device_name("扬声器 (DUNU DTC100pro)", &cfg),
            "我的DAC"
        );
        assert_eq!(resolve_device_name("DUNU DTC100pro", &cfg), "我的DAC");
    }

    /// ⭐ **从设备页改名（原名 = 短名）⇒ 同样两种形态都落入配置**（对称性）。
    #[test]
    fn apply_device_rename_from_device_page_writes_both_forms() {
        let mut cfg = Config::default();

        // `core_name("DUNU DTC100pro")` == 自身 ⇒ 两次 insert 落到同一个键上
        assert!(apply_device_rename(&mut cfg, "DUNU DTC100pro", "我的DAC"));

        assert_eq!(
            cfg.device_names.get("DUNU DTC100pro").map(String::as_str),
            Some("我的DAC")
        );
        assert_eq!(
            cfg.device_names.len(),
            1,
            "短名形态只需一条键，实际 {cfg:?}"
        );
        assert_eq!(resolve_device_name("DUNU DTC100pro", &cfg), "我的DAC");
        // 另一侧的带括号原串**也能**解析到（靠读取侧第 1 级 `core_name`）
        assert_eq!(
            resolve_device_name("扬声器 (DUNU DTC100pro)", &cfg),
            "我的DAC",
            "即使只写了短名键，音量页的带括号原串也必须能解析到"
        );
    }

    /// **恢复默认（`new_name` 为空）⇒ 两种形态都删净**。
    /// ⛔ 若只删 `original`，短名键会残留 ⇒ 用户点「恢复默认」后发现名字**没变回去**。
    #[test]
    fn apply_device_rename_clears_both_forms_on_reset() {
        let mut cfg = Config::default();
        apply_device_rename(&mut cfg, "扬声器 (DUNU DTC100pro)", "我的DAC");
        assert_eq!(cfg.device_names.len(), 2);

        assert!(
            apply_device_rename(&mut cfg, "扬声器 (DUNU DTC100pro)", ""),
            "删除必须报告 changed"
        );

        assert!(
            cfg.device_names.is_empty(),
            "两种形态必须都删净，实际 {cfg:?}"
        );
        assert_eq!(
            resolve_device_name("DUNU DTC100pro", &cfg),
            "DUNU DTC100pro",
            "恢复默认后必须回落到原名"
        );
    }

    /// **`new_name == original` 也视为恢复默认**（与旧实现的语义一致）。
    #[test]
    fn apply_device_rename_treats_same_name_as_reset() {
        let mut cfg = Config::default();
        apply_device_rename(&mut cfg, "扬声器 (DUNU DTC100pro)", "我的DAC");

        assert!(apply_device_rename(
            &mut cfg,
            "扬声器 (DUNU DTC100pro)",
            "扬声器 (DUNU DTC100pro)"
        ));
        assert!(cfg.device_names.is_empty(), "实际 {cfg:?}");
    }

    // ── 固定项自带别名（`PinnedDevice.alias`）折进全局改名表 ──────────

    /// ⭐ 旧配置里带 alias 的固定项 ⇒ 别名必须对**所有表面**可见，
    /// 否则任务栏 tooltip 一个名、别处另一个名（这正是要修的不一致）。
    #[test]
    fn fold_pinned_alias_becomes_a_global_device_name() {
        // ⚠️ 用结构体字面量而非「`default()` 之后再赋字段」——
        //    后者会命中 clippy::field_reassign_with_default（闸门按 -D warnings 拦截）。
        let mut cfg = Config {
            pinned_taskbar_devices: vec![PinnedDevice {
                key: "c:abc".to_string(),
                // `fallback` = `n:<core_name(显示名)>`，与 `resolved_display_name` 同算法
                fallback: Some("n:DUNU DTC100pro".to_string()),
                alias: Some("我的DAC".to_string()),
            }],
            ..Config::default()
        };

        assert!(fold_pinned_alias_into_device_names(&mut cfg));

        assert_eq!(
            cfg.device_names.get("DUNU DTC100pro").map(String::as_str),
            Some("我的DAC")
        );
        // ⛔ 折叠后**任何表面**都必须解析到同一个名字（此处以共享判据为准）
        assert_eq!(resolve_device_name("DUNU DTC100pro", &cfg), "我的DAC");
        assert_eq!(
            resolve_device_name("扬声器 (DUNU DTC100pro)", &cfg),
            "我的DAC",
            "音频端点原串也必须解析到（音量页/托盘靠这条）"
        );
    }

    /// ⛔ **不得覆盖**已有的 `device_names` 条目：用户后来在别处改过名就以那个为准；
    /// 空 alias / 无 `n:` 兜底键 / 非空白的兜底键都不折叠。
    #[test]
    fn fold_pinned_alias_respects_existing_and_ignores_garbage() {
        let mut cfg = Config {
            device_names: [("DUNU DTC100pro".to_string(), "用户后来改的名".to_string())]
                .into_iter()
                .collect(),
            pinned_taskbar_devices: vec![
                // ① 已有条目 ⇒ 不覆盖
                PinnedDevice {
                    key: "c:abc".to_string(),
                    fallback: Some("n:DUNU DTC100pro".to_string()),
                    alias: Some("旧别名".to_string()),
                },
                // ② alias 为空白 ⇒ 跳过
                PinnedDevice {
                    key: "c:def".to_string(),
                    fallback: Some("n:X".to_string()),
                    alias: Some("   ".to_string()),
                },
                // ③ alias 为 None ⇒ 跳过
                PinnedDevice {
                    key: "c:ghi".to_string(),
                    fallback: Some("n:Y".to_string()),
                    alias: None,
                },
                // ④ 没有 `n:` 形态的兜底键 ⇒ 无从得知挂哪个短名，跳过
                PinnedDevice {
                    key: "c:jkl".to_string(),
                    fallback: Some(r"i:USB\VID_1".to_string()),
                    alias: Some("不该出现".to_string()),
                },
            ],
            ..Config::default()
        };

        // 返回 true：① 号条目的 alias 被**清空**（这一级历史层级就此退休）
        assert!(fold_pinned_alias_into_device_names(&mut cfg));

        assert_eq!(
            cfg.device_names.get("DUNU DTC100pro").map(String::as_str),
            Some("用户后来改的名"),
            "已有条目不得被 alias 覆盖"
        );
        assert!(
            !cfg.device_names.contains_key("X")
                && !cfg.device_names.contains_key("Y")
                && !cfg.device_names.contains_key(r"i:USB\VID_1"),
            "空/None/非 n: 前缀的条目都不得写入，实际 {:?}",
            cfg.device_names
        );

        // ① 已有全局条目 ⇒ alias 仍要清：留着它会让任务栏继续显示旧名（P0）
        assert_eq!(
            cfg.pinned_taskbar_devices[0].alias, None,
            "alias 必须退休，否则它作为更高优先级把任务栏拉回旧名"
        );
        // ② 无从迁移（空白别名）⇒ 原样保留
        assert_eq!(cfg.pinned_taskbar_devices[1].alias.as_deref(), Some("   "));
        // ③ 本来就是 None
        assert_eq!(cfg.pinned_taskbar_devices[2].alias, None);
        // ④ 算不出短名 ⇒ **保留** alias：宁可留不一致，也不能把用户起的名丢掉
        assert_eq!(
            cfg.pinned_taskbar_devices[3].alias.as_deref(),
            Some("不该出现"),
            "迁移不了时必须原样保留"
        );
    }

    /// ⭐⭐ P0 的核心防线：`pin.alias` 折叠后必须清空。
    ///
    /// ⛔ 可证伪：去掉 `to_clear` 那段（只折进 `device_names`、保留 alias），
    ///   本测试转红 —— 届时用户全局改名后任务栏显示旧 alias、别处显示新名。
    #[test]
    fn fold_pinned_alias_retires_the_alias_level() {
        let mut cfg = Config {
            pinned_taskbar_devices: vec![PinnedDevice {
                key: "c:abc".to_string(),
                fallback: Some("n:小爱音箱-9205".to_string()),
                alias: Some("旧别名".to_string()),
            }],
            ..Config::default()
        };

        assert!(fold_pinned_alias_into_device_names(&mut cfg));

        assert_eq!(
            cfg.device_names.get("小爱音箱-9205").map(String::as_str),
            Some("旧别名")
        );
        assert!(
            cfg.pinned_taskbar_devices[0].alias.is_none(),
            "alias 必须清空：它比 device_names 优先级更高，留着就压住了全局改名"
        );
        // 清空后任务栏与其它表面走**同一条**判据
        assert_eq!(
            crate::device_identity::resolved_display_name(
                "小爱音箱-9205",
                "c:abc",
                &cfg.pinned_taskbar_devices,
                &cfg
            ),
            "旧别名"
        );
    }

    /// `resolve_device_name_in`（只吃改名表）必须与 `resolve_device_name`（吃整份
    /// Config）**逐字同判** —— 前者是 `battery_notify` 持表快照时的入口。
    #[test]
    fn resolve_device_name_in_matches_the_config_variant() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), "我的DAC".to_string());

        for raw in ["DUNU DTC100pro", "扬声器 (DUNU DTC100pro)", "别的设备"] {
            assert_eq!(
                resolve_device_name(raw, &cfg),
                resolve_device_name_in(raw, &cfg.device_names),
                "两种入口对 {raw:?} 必须一致"
            );
        }
    }

    // ── A：空白别名必须当「无别名」 ──────────────────────────

    /// ⛔ A 的可证伪判据：`get()` 命中空串/纯空白时必须**回落原名**。
    ///   修复前 `resolve_device_name_in` 会 `return ""` ⇒ 任务栏 tooltip /
    ///   托盘菜单 / 低电量通知显示**空白**，而四个页面（JS 的 `if (custom)`
    ///   判假）正常显示原名 ⇒ 同一设备两套名字。
    #[test]
    fn blank_alias_falls_back_to_the_raw_name() {
        for blank in [
            "", "   ", "	
",
        ] {
            let mut cfg = Config::default();
            cfg.device_names
                .insert("DUNU DTC100pro".to_string(), blank.to_string());
            cfg.device_names
                .insert("扬声器 (DUNU DTC100pro)".to_string(), blank.to_string());

            assert_eq!(
                resolve_device_name("DUNU DTC100pro", &cfg),
                "DUNU DTC100pro",
                "空白别名（{blank:?}）必须回落原名"
            );
            assert_eq!(
                resolve_device_name("扬声器 (DUNU DTC100pro)", &cfg),
                "扬声器 (DUNU DTC100pro)",
                "空白别名（{blank:?}）必须回落原名"
            );
        }
    }

    /// 空白别名不得**遮蔽**另一个形态的有效别名：短名键空、长形态键有值时，
    /// 应回退到长形态那条，而不是直接返回原名。
    #[test]
    fn blank_alias_does_not_shadow_a_valid_one() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), "  ".to_string());
        cfg.device_names
            .insert("扬声器 (DUNU DTC100pro)".to_string(), "我的DAC".to_string());

        assert_eq!(
            resolve_device_name("DUNU DTC100pro", &cfg),
            "我的DAC",
            "短名键是空白时应继续回退到长形态的有效别名"
        );
    }

    // ── B：显示名长度上限 ────────────────────────────────────

    /// ⭐ 超长显示名必须被截断并补省略号，且**按字符而非字节**——
    ///   按字节砍会把汉字劈开（`chars().count()` 才是「几个字」）。
    #[test]
    fn clamp_display_name_counts_chars_not_bytes() {
        assert_eq!(clamp_display_name("短名"), "短名");
        let exact: String = "字".repeat(MAX_DISPLAY_NAME_CHARS);
        assert_eq!(clamp_display_name(&exact), exact, "恰好到上限不应截断");

        let over: String = "字".repeat(MAX_DISPLAY_NAME_CHARS + 10);
        let out = clamp_display_name(&over);
        assert_eq!(out.chars().count(), MAX_DISPLAY_NAME_CHARS);
        assert!(out.ends_with('…'), "截断后必须补省略号，实际 {out:?}");

        let ascii = "a".repeat(MAX_DISPLAY_NAME_CHARS + 1);
        assert_eq!(
            clamp_display_name(&ascii).chars().count(),
            MAX_DISPLAY_NAME_CHARS
        );
    }

    /// ⭐ 截断只发生在**渲染前**，重命名对话框的初值（走 `resolve_device_name`）
    ///   必须仍是完整别名 —— 否则用户编辑长名字会存回一个被削过的值。
    #[test]
    fn resolve_device_name_never_truncates() {
        let long_alias = "超长别名".repeat(40);
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), long_alias.clone());

        assert_eq!(
            resolve_device_name("DUNU DTC100pro", &cfg),
            long_alias,
            "解析函数不得截断（重命名对话框的初值走它）"
        );
        assert_eq!(
            crate::device_identity::resolved_display_name("DUNU DTC100pro", "c:abc", &[], &cfg)
                .chars()
                .count(),
            MAX_DISPLAY_NAME_CHARS,
            "渲染层才截断"
        );
    }

    // ── C：写入时归并**所有**已存在的同 core_name 键 ──────────

    /// ⛔ C 的可证伪判据：从**短名**入口改名时，配置里已存在的**长形态**键
    ///   也必须被更新。修复前它保持旧值 ⇒ 配置里长期躺着互相矛盾的两条别名。
    #[test]
    fn rename_heals_every_existing_form_of_the_same_device() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("扬声器 (DUNU DTC100pro)".to_string(), "旧名A".to_string());
        cfg.device_names
            .insert("耳机 (DUNU DTC100pro)".to_string(), "旧名B".to_string());

        assert!(apply_device_rename(&mut cfg, "DUNU DTC100pro", "我的DAC"));

        for k in [
            "DUNU DTC100pro",
            "扬声器 (DUNU DTC100pro)",
            "耳机 (DUNU DTC100pro)",
        ] {
            assert_eq!(
                cfg.device_names.get(k).map(String::as_str),
                Some("我的DAC"),
                "形态 {k:?} 未被归并，实际 {:?}",
                cfg.device_names
            );
        }
    }

    /// ⛔ 归并**不得**波及其他设备：不同 `core_name` 的键必须原样保留。
    #[test]
    fn rename_heal_does_not_touch_other_devices() {
        let mut cfg = Config::default();
        cfg.device_names
            .insert("DUNU DTC100pro".to_string(), "我的DAC".to_string());
        cfg.device_names
            .insert("OPPO Enco X".to_string(), "别人的名".to_string());

        apply_device_rename(&mut cfg, "DUNU DTC100pro", "新DAC");

        assert_eq!(
            cfg.device_names.get("OPPO Enco X").map(String::as_str),
            Some("别人的名"),
            "别的设备不得被波及，实际 {:?}",
            cfg.device_names
        );
    }

    /// **重复写同一个名字 ⇒ 第二次报告「无变化」**（避免落盘层误判为配置变更）。
    #[test]
    fn apply_device_rename_is_idempotent() {
        let mut cfg = Config::default();
        assert!(apply_device_rename(&mut cfg, "扬声器 (X)", "新名"));
        assert!(
            !apply_device_rename(&mut cfg, "扬声器 (X)", "新名"),
            "内容未变时不得报告 changed"
        );
        assert_eq!(cfg.device_names.len(), 2);
    }

    /// ⛔ **反向注入靶子**：把归并删除/写入任一半拆掉 ⇒ 上面几条必须转红。
    /// 本用例额外钉住「两半都不能少」这一点（用一个非对称场景）。
    #[test]
    fn apply_device_rename_covers_both_directions_asymmetrically() {
        let mut cfg = Config::default();
        // 从音量页改名 → 再从设备页改回默认 ⇒ 必须全清空，不能只清一半
        apply_device_rename(&mut cfg, "扬声器 (DUNU DTC100pro)", "我的DAC");
        apply_device_rename(&mut cfg, "DUNU DTC100pro", "");

        assert!(
            cfg.device_names.is_empty(),
            "从设备页恢复默认也必须清掉音量页那条键，实际 {cfg:?}"
        );
    }
}

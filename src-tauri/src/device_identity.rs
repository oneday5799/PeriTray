// ── 模块职责 ─────────────────────────────────────────────
// 物理设备身份：把「跨设备类别的节点」归并到同一台物理设备。
//
// 背景：Windows PnP 把同一物理设备的所有功能节点（USB 复合接口、各 HID 集合、
// 蓝牙服务节点、音频端点）归到同一个 ContainerId ⇒ 它是唯一能跨类别认出
// 「这是同一台设备」的键。实测本机 278 个 devnode：2.4G 接收器 10 节点 → 1 台、
// BLE 键鼠（BTHLE\Dev_<mac> + HID\{00001812-…}）同容器、Razer 鼠标 19 节点 → 1 台。
//
// 键优先级（降级链）：ContainerId → PnP 实例路径 → 名称。
// 之所以要降级链：容器会因换机/重装系统而变，虚拟设备则压根没有真实容器。
//
// 本模块只做身份判定，不持有状态、不做缓存、不碰锁。

use crate::dedup::core_name;
use crate::device::DevType;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Get_Device_ID_ListW, CM_Get_Device_ID_List_SizeW,
    CM_Locate_DevNodeW, CM_GETIDLIST_FILTER_ENUMERATOR, CM_LOCATE_DEVNODE_NORMAL,
};
use windows_sys::Win32::Devices::Properties::DEVPKEY_Device_ContainerId;

/// 占位容器 GUID（小写、无花括号）。**必须视为「无容器」**。
///
/// 实测 `{…-ffffffffffff}` 下聚集 107 个互不相关节点（ACPI / HID / ROOT / SCSI /
/// SWD / USB 混在一起，含 `USB\ROOT_HUB30`、`HID\GVInput&Col0x`）；更关键的是本机
/// **3 个互不相关的虚拟音频设备**（网易虚拟音频 render / 网易 capture /
/// Steam Streaming Speakers）**共享同一占位容器** ⇒ 不排除会把它们错误合并成
/// 「一台」设备。全零容器同理。
pub const NULL_CONTAINERS: [&str; 2] = [
    "00000000-0000-0000-0000-000000000000",
    "00000000-0000-0000-ffff-ffffffffffff",
];

/// 归一化容器 GUID：去空白、剥花括号、转小写。
///
/// ⚠️ 这一步是**判据的一部分**，不是美化：实测音频侧常输出「无花括号小写」而
/// devnode 侧是「带花括号 REG_SZ」，字符串直接比较**恒不等** ⇒ 曾得到
/// 「6/6 全部未命中」的**假阴性**（期望值没错，错的是两侧取材/归一化不一致）。
pub fn normalize_guid(raw: &str) -> String {
    raw.trim()
        .trim_matches(|c| c == '{' || c == '}')
        .trim()
        .to_ascii_lowercase()
}

/// 是否为占位容器（入参可带花括号、大小写任意）。
pub fn is_null_container(guid: &str) -> bool {
    let g = normalize_guid(guid);
    NULL_CONTAINERS.iter().any(|n| *n == g)
}

/// 判定「可用容器」：缺失 / 空串 / 占位容器一律返回 `None`，调用方**必须降级**。
pub fn usable_container(raw: Option<&str>) -> Option<String> {
    let g = normalize_guid(raw?);
    if g.is_empty() || is_null_container(&g) {
        return None;
    }
    Some(g)
}

/// 物理设备身份键。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum DeviceKey {
    /// 真实容器（已归一化：小写、无花括号）
    Container(String),
    /// 降级一：PnP 实例路径（如 `USB\VID_046D&PID_C092\5&1A2B3C4D&0&1`）
    Instance(String),
    /// 降级二：显示名（等同今天的语义，最后兜底）
    Name(String),
}

impl DeviceKey {
    /// 编码为单一字符串，供持久化 / 前端使用。
    /// 带前缀是为了让来源可辨、且**跨级不会碰撞**（同名设备与同名实例不会混）。
    pub fn encode(&self) -> String {
        match self {
            Self::Container(g) => format!("c:{g}"),
            Self::Instance(i) => format!("i:{}", i.to_ascii_lowercase()),
            Self::Name(n) => format!("n:{n}"),
        }
    }
}

/// 按降级链构造身份键。三者皆空返回 `None`（调用方应丢弃该节点）。
pub fn device_key(
    container_raw: Option<&str>,
    instance: Option<&str>,
    name: Option<&str>,
) -> Option<DeviceKey> {
    if let Some(g) = usable_container(container_raw) {
        return Some(DeviceKey::Container(g));
    }
    if let Some(i) = instance.map(str::trim).filter(|s| !s.is_empty()) {
        return Some(DeviceKey::Instance(i.to_string()));
    }
    let n = name.map(str::trim).filter(|s| !s.is_empty())?;
    Some(DeviceKey::Name(n.to_string()))
}

/// 16 字节 GUID 缓冲区 → 小写无花括号字符串。
///
/// 前三个字段是小端存储（`Data1` 4 字节 / `Data2` 2 字节 / `Data3` 2 字节），
/// 后两个字段按字节序 ⇒ 必须逐字节反序前 4/2/2 字节，否则得到伪 GUID。
fn format_guid_bytes(b: &[u8]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-\
         {:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3],
        b[2],
        b[1],
        b[0],
        b[5],
        b[4],
        b[7],
        b[6],
        b[8],
        b[9],
        b[10],
        b[11],
        b[12],
        b[13],
        b[14],
        b[15]
    )
}

/// 从 PnP 实例路径读容器 GUID（CFGMGR32）。
///
/// 失败（定位不到 / 属性缺失 / 占位容器）一律返回 `None` ⇒ 调用方降级。
/// ⚠️ `CM_Get_DevNode_PropertyW` **首次调用只为取长度，必然返回
/// `CR_BUFFER_SMALL`(0x1A)，不能当成错误** —— 这是本 API 最常见的误用点。
pub fn container_of_instance(instance: &str) -> Option<String> {
    let wide: Vec<u16> = instance.encode_utf16().chain(std::iter::once(0)).collect();
    let mut devinst: u32 = 0;
    // SAFETY: wide 以 NUL 结尾且在整个调用期间存活；devinst 是栈上可写的 u32。
    let cr = unsafe { CM_Locate_DevNodeW(&mut devinst, wide.as_ptr(), CM_LOCATE_DEVNODE_NORMAL) };
    if cr != 0 {
        return None;
    }

    let mut property_type: u32 = 0;
    let mut size: u32 = 0;
    // SAFETY: 传空缓冲区 + 0 长度是「只取长度」的官方用法，返回值此处有意忽略。
    unsafe {
        CM_Get_DevNode_PropertyW(
            devinst,
            &DEVPKEY_Device_ContainerId,
            &mut property_type,
            std::ptr::null_mut(),
            &mut size,
            0,
        );
    }
    // 容器恒为 GUID（16 字节）；小于 16 说明属性不存在。
    if size < 16 {
        return None;
    }

    let mut buf = [0u8; 16];
    let mut size = 16u32;
    // SAFETY: buf 恰为 16 字节且 size 与之匹配，函数不会越界写。
    let cr = unsafe {
        CM_Get_DevNode_PropertyW(
            devinst,
            &DEVPKEY_Device_ContainerId,
            &mut property_type,
            buf.as_mut_ptr(),
            &mut size,
            0,
        )
    };
    if cr != 0 {
        return None;
    }
    usable_container(Some(&format_guid_bytes(&buf)))
}

/// 音频端点 id（`IMMDevice::GetId()` 的返回值）→ 容器 GUID。
///
/// 实测音频端点的 devnode 实例名就是 `SWD\MMDEVAPI\{id}`，且其
/// `DEVPKEY_Device_ContainerId` 与同一物理设备的 `BTHENUM\Dev_<MAC>` 节点**逐字相等**
/// ⇒ **不需要读 COM `IPropertyStore`、不需要解析 MAC**。
pub fn container_of_audio_endpoint(endpoint_id: &str) -> Option<String> {
    container_of_instance(&format!("SWD\\MMDEVAPI\\{endpoint_id}"))
}

/// hidapi 设备**接口**路径 → devnode **实例**路径。
///
/// hidapi（Windows）给的是接口路径，CFGMGR32 要的是实例路径，两者形如：
///
/// ```text
/// hidapi : \\?\HID#VID_1532&PID_0094&MI_01&Col07#8&b16f3a&0&0006#{4d1e55b2-…}\KBD
/// devnode: HID\VID_1532&PID_0094&MI_01&COL07\8&b16f3a&0&0006
/// ```
///
/// 变换规则（实机验证见下）：
///   1. 剥掉开头的 `\\?\`（或 `\\.\`）；
///   2. **从最后一个 `#{` 处截断** —— 那是接口类 GUID 后缀。
///      ⚠️ **不能用第一个 `#{`**：蓝牙 HID 的设备 ID 段本身以 `{00001812-…}` 开头
///      （形如 `HID#{00001812-…}_Dev_VID&021532_…`），按第一个切会把设备 ID 段整段丢掉；
///      同时这一步也顺带丢掉了尾部偶发的 `\KBD` 后缀（实机见过）；
///   3. `#` → `\`。
///
/// **不做任何大小写变换**：hidapi 侧本就把枚举器/VID/PID/MI 段输出为大写，
/// 只有 `Col07` 这类集合名与 devnode 的 `COL07` 不同；而 Windows 设备实例 ID
/// **大小写不敏感** ⇒ 交给 `CM_Locate_DevNodeW` 匹配即可。
/// 刻意不转大写是为了不破坏蓝牙 HID 设备 ID 段里 `{00001812-…}_Dev_VID&…_c6947e50a677`
/// 那种「大小写混合且必须逐字匹配（对注册表而言）」的形态。
///
/// ⭐ **实机验证（2026-09-21，Razer Orochi V2 `1532:0094`）**：hidapi 枚举到的
/// **12 个集合全部**解析出容器，且与设备自身容器
/// `40e11c06-72bd-5b38-9bd2-0e15079b3b45` 一致（分域后仍是 12 个，一个不少）
/// ⇒ 映射与容器解析在真机上 **12/12** 成立。
/// （更早一次探针曾记录「27/27」，但该探针已删除、**未能复核**，故不引用。）
///
/// ⚠️ **诚实边界**：`HID#{GUID}_Dev_…` 这一形态本机未接设备 ⇒ 其
/// `CM_Locate_DevNodeW` 命中**未经实测**，属结构外推；已由单测钉住结构变换。
pub fn devnode_from_hidapi_path(path: &str) -> Option<String> {
    let s = path
        .strip_prefix(r"\\?\")
        .or_else(|| path.strip_prefix(r"\\.\"))
        .unwrap_or(path);
    let s = match s.rfind("#{") {
        Some(i) => &s[..i],
        None => s,
    };
    let mut segs: Vec<&str> = s.split('#').collect();
    if segs.len() < 3 {
        return None;
    }
    segs.truncate(3);
    let (head, dev, inst) = (segs[0], segs[1], segs[2]);
    if head.is_empty() || dev.is_empty() || inst.is_empty() {
        return None;
    }
    Some(format!("{head}\\{dev}\\{inst}"))
}

/// 枚举指定枚举器下的**设备实例路径**（CFGMGR32 `CM_Get_Device_ID_ListW`）。
///
/// ⚠️ **本函数返回的列表包含「非在场（phantom）」设备** —— 只传了
/// `CM_GETIDLIST_FILTER_ENUMERATOR`，**没有**叠加 `CM_GETIDLIST_FILTER_PRESENT`。
/// 实测（2026-09-21）：HID `ENUMERATOR` = **87** 条，而 `ENUMERATOR|PRESENT` = **29** 条；
/// 差集 58 条用 `CM_LOCATE_DEVNODE_NORMAL` **全部失败**、用 `..._PHANTOM` **全部成功**
/// ⇒ 「非在场」这一解释成立（不是定位调用写错）。USB 51 vs 18、SWD 36 vs 30 同理。
///
/// ✅ **当前无影响**：唯一调用方 `bluetooth_container_map()` 只用 `BTHENUM` / `BTHLE`，
/// 而这两个枚举器上两者结果**完全相同**（14 vs 14、3 vs 3，差集 0）。
/// ⛔ **但这颗雷要记住**：若日后有人拿本函数去枚举 `HID` / `USB` / `SWD`，
/// 会拿到**一批非在场设备**，且它们 `CM_Locate_DevNodeW(NORMAL)` 必然失败 ——
/// 表现为「映射莫名缺失」，很难定位到根因。
/// 是否加 `..._PRESENT` 属独立决策（加了会改变语义），本批**未动**。
///
/// 失败一律返回空表 —— 调用方应把它当成「没有可用映射」，而不是错误。
fn enumerator_instance_ids(enumerator: &str) -> Vec<String> {
    // 过滤器是 MULTI_SZ：单个枚举器名 + **双 NUL** 结尾。
    let filter: Vec<u16> = enumerator.encode_utf16().chain([0u16, 0u16]).collect();

    let mut size: u32 = 0;
    // SAFETY: filter 以双 NUL 结尾且在本函数内存活；size 是栈上可写 u32。
    let cr = unsafe {
        CM_Get_Device_ID_List_SizeW(&mut size, filter.as_ptr(), CM_GETIDLIST_FILTER_ENUMERATOR)
    };
    if cr != 0 || size == 0 {
        return Vec::new();
    }

    let mut buf = vec![0u16; size as usize];
    // SAFETY: buf 长度恰为 size（上面刚查出来的），函数不会越界写。
    let cr = unsafe {
        CM_Get_Device_ID_ListW(
            filter.as_ptr(),
            buf.as_mut_ptr(),
            size,
            CM_GETIDLIST_FILTER_ENUMERATOR,
        )
    };
    if cr != 0 {
        return Vec::new();
    }

    // MULTI_SZ：条目以 NUL 分隔、以空条目（双 NUL）收尾，其后是补零。
    let mut out = Vec::new();
    for chunk in buf.split(|&c| c == 0) {
        if chunk.is_empty() {
            break;
        }
        out.push(String::from_utf16_lossy(chunk));
    }
    out
}

/// 从蓝牙 PnP 实例路径抠出大写 MAC（无分隔符）。
///
/// 本机实测存在两种形态，**两种都要试**：
///   · 设备节点：`BTHENUM\Dev_5088112E80E8\8&1d39e19e&0&BluetoothDevice_5088112E80E8`
///     —— 经典蓝牙与 BLE 都有此形态（`BTHLE\Dev_<mac>\…` 同构，仅大小写不同）
///   · 服务节点：`BTHENUM\{0000110b-…}_VID&…\8&1d39e19e&0&5088112E80E8_C00000000`
///
/// ⚠️ **绝不能用「第一个 12 位十六进制串」**：服务节点形态下它会命中 A2DP 服务 GUID 里的
/// `00805F9B34FB` —— 本机实测该错误取法在 3/3 服务节点上**全部**返回这个值。
pub fn mac_from_bluetooth_instance(instance: &str) -> Option<String> {
    // 实例路径恒为 ASCII；非 ASCII 直接放弃，避免下面的字节下标切片踩到字符边界。
    if !instance.is_ascii() {
        return None;
    }
    let upper = instance.to_ascii_uppercase();

    // 形态一：`Dev_<MAC>`
    if let Some(i) = upper.find("DEV_") {
        if let Some(mac) = hex12_at(&upper, i + 4) {
            return Some(mac);
        }
    }
    // 形态二：`&0&<MAC>_`（取最后一次出现，紧邻 MAC）
    if let Some(i) = upper.rfind("&0&") {
        if let Some(mac) = hex12_at(&upper, i + 3) {
            return Some(mac);
        }
    }
    None
}

/// 取 `s[start..start+12]`，要求这 12 个字符**全是十六进制**，否则 `None`。
fn hex12_at(s: &str, start: usize) -> Option<String> {
    let end = start.checked_add(12)?;
    let seg = s.get(start..end)?;
    if seg.bytes().all(|b| b.is_ascii_hexdigit()) {
        Some(seg.to_ascii_uppercase())
    } else {
        None
    }
}

/// 建立「大写 MAC → 容器 GUID」映射（覆盖经典蓝牙与 BLE）。
///
/// ⭐ 实测：同一 MAC 会命中 **3~4 个** BTHENUM 服务实例（A2DP / AVRCP / HFP…），
/// 但它们的容器**完全一致** ⇒ 可以安全地建单值映射，不会因多实例产生歧义。
/// 这使「蓝牙设备（WinRT 来源，只有 device_id）↔ 音频端点」得以落到同一容器。
pub fn bluetooth_container_map() -> HashMap<String, String> {
    let mut map: HashMap<String, String> = HashMap::new();
    for enumerator in ["BTHENUM", "BTHLE"] {
        for instance in enumerator_instance_ids(enumerator) {
            let Some(mac) = mac_from_bluetooth_instance(&instance) else {
                continue;
            };
            if map.contains_key(&mac) {
                continue;
            }
            if let Some(container) = container_of_instance(&instance) {
                map.insert(mac, container);
            }
        }
    }
    map
}

/// 电量来源。**数值越大越可信** —— 同一容器内出现多个来源时取最大的那个。
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum BatterySource {
    /// 来源不明（例如 `Win32_Battery` 的估算值）
    Unknown = 0,
    /// 2.4G 接收器经 HID Feature Report 读得
    Hid24g = 1,
    /// 蓝牙属性 `DEVPKEY_Device_BatteryLevel`
    Bluetooth = 2,
}

/// 一台**物理设备** —— 把同一容器下的各功能节点聚合后的结果。
///
/// 这是任务栏信息窗的数据单元：电量来自蓝牙属性 / HID，音量来自该设备的**输出**端点。
///
/// ⚠️ **「置灰占位」条目的判据**：`battery` 与 `audio_device_id` **皆为 `None`** 时，
/// 这条没有任何可显示的数据 —— 它只可能来自「被用户固定、但此刻枚举不到（未连接 /
/// 未插）」的反向补建。前端应**置灰**呈现而非隐藏：用户 pin 了却看不见，会以为设置丢了。
/// 判据请写 `battery == null && audio_device_id == null`（单字段用 `!= null`），
/// **不要写 `!d.battery`** —— `0%` 是合法电量，`!0` 为真会把电量耗尽的设备误判成无数据。
#[derive(Debug, Clone, Serialize)]
pub struct PhysicalDevice {
    /// `DeviceKey::encode()` 的结果（`c:` / `i:` / `n:` 前缀）
    pub key: String,
    /// 展示名，由组内候选名按 `pick_display_name` 挑出；占位条目由 `pinned_placeholder_name` 给
    pub name: String,
    /// 电量百分比。⚠️ `Some(0)` 是**合法**的（电量耗尽），与 `None`（读不出）语义完全不同
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<i32>,
    /// 该物理设备当前的**输出**端点 id（供音量使用）；无音频端点时为 `None`（如键鼠）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_device_id: Option<String>,
    /// 该容器出现过的设备类别（一个容器可跨多类，实测 USB + HID + SWD）；占位条目为空数组
    pub categories: Vec<DevType>,
    /// 参与聚合的条目数（设备行 + 音频端点行）；占位条目为 0。
    ///
    /// ⚠️ 这**不是** devnode 数：PeriTray 的设备列表在 WMI 层已过滤掉大部分子节点
    /// （`is_generic_hid` 滤 `&COL*`、`is_bt_service` 滤蓝牙服务节点），所以这里通常很小。
    /// 「10 个 devnode 并成 1 台」是 ContainerId 在 devnode 层面的性质，不由此字段体现。
    pub node_count: usize,
    /// 是否被用户固定（由 config 决定，`Grouper` 本身不关心）。
    ///
    /// ⚠️ 语义是「**强制显示**」：为真的条目即使此刻无数据也会保留
    /// （见 `group_taskbar_devices` 的保留规则与补建循环），由前端置灰。
    pub pinned: bool,
}

/// 物理设备分组器。
///
/// 以**容器为根**聚合，而不是以设备类别为根 —— 需求已含键鼠，音频端点不是必经之路。
/// 输出按 key 排序（内部 `BTreeMap`）⇒ 顺序稳定，不会因枚举顺序抖动而让 UI 跳动。
#[derive(Default)]
pub struct Grouper {
    acc: BTreeMap<String, Group>,
}

#[derive(Default)]
struct Group {
    names: Vec<String>,
    battery: Option<(i32, BatterySource)>,
    audio_device_id: Option<String>,
    categories: Vec<DevType>,
    node_count: usize,
}

impl Grouper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入一个节点。`key` 为 `None` 的节点**直接丢弃** —— 没有身份的设备不该出现在任务栏。
    pub fn add(
        &mut self,
        key: Option<&str>,
        name: &str,
        dt: DevType,
        battery: Option<(i32, BatterySource)>,
        audio_device_id: Option<&str>,
    ) {
        let Some(key) = key else { return };
        let g = self.acc.entry(key.to_string()).or_default();

        if !name.trim().is_empty() {
            g.names.push(name.to_string());
        }
        if let Some((level, source)) = battery {
            // 取来源更可信的那个；同源时保留先到的（枚举顺序内的稳定选择）
            let take = match g.battery {
                Some((_, current)) => source > current,
                None => true,
            };
            if take {
                g.battery = Some((level, source));
            }
        }
        if let Some(id) = audio_device_id {
            if g.audio_device_id.is_none() {
                g.audio_device_id = Some(id.to_string());
            }
        }
        if !g.categories.contains(&dt) {
            g.categories.push(dt);
        }
        g.node_count += 1;
    }

    pub fn finish(self) -> Vec<PhysicalDevice> {
        self.acc
            .into_iter()
            .map(|(key, g)| PhysicalDevice {
                key,
                name: pick_display_name(&g.names),
                battery: g.battery.map(|(level, _)| level),
                audio_device_id: g.audio_device_id,
                categories: g.categories,
                node_count: g.node_count,
                pinned: false,
            })
            .collect()
    }
}

/// 从组内候选名里挑展示名。
///
/// 规则（确定性、可测、与输入顺序无关）：
///   1. 每个候选先过 `dedup::core_name`（取括号内 + 剥协议后缀）。实测音频端点的
///      `FriendlyName` 形如 `耳机 (小爱音箱-9205)`，这一步能把「耳机」这种**毫无区分度**
///      的名字换成真正的物理设备名；
///   2. 去重后取**最长**者（更具体的名字通常更长）；
///   3. 同长时取字典序最小者（`reduce` 保留首个最大值，而候选已排序）⇒ 结果稳定。
fn pick_display_name(names: &[String]) -> String {
    let mut cands: Vec<String> = names
        .iter()
        .map(|n| core_name(n))
        .filter(|n| !n.trim().is_empty())
        .collect();
    cands.sort();
    cands.dedup();
    cands
        .into_iter()
        .reduce(|best, cur| {
            if cur.chars().count() > best.chars().count() {
                cur
            } else {
                best
            }
        })
        .unwrap_or_else(|| "未知设备".to_string())
}

/// 音频端点 → 身份键。有容器用容器；**没有容器时退回端点自己的 devnode 实例路径**
/// （`SWD\MMDEVAPI\{id}`），这样它仍能与 PnP 侧同一端点的 `Device.device_key` 对齐
/// —— 虚拟音频设备正走这条路（它们落在占位容器上，没有真实容器）。
fn audio_endpoint_key(audio: &crate::audio::AudioDevice) -> DeviceKey {
    match &audio.container_id {
        Some(c) => DeviceKey::Container(c.clone()),
        None => DeviceKey::Instance(format!("SWD\\MMDEVAPI\\{}", audio.id)),
    }
}

/// 把设备列表与音频端点列表按物理设备身份聚合，产出任务栏信息窗的数据源。
///
/// 保留规则（**固定优先**）：
///   · 有电量**或**有音量 ⇒ 保留；两者皆无且**未**被固定 ⇒ 丢弃（对用户毫无意义，
///     典型是落在占位容器上的虚拟音频设备）；
///   · **被固定（pin）⇒ 一律保留**，即使此刻读不出任何数据。固定是用户的显式意图，
///     「pin 了却看不见」会让用户以为设置丢了；这类条目由前端**置灰**呈现。
///
/// ⚠️ 「强制显示」由**两条互补**的路径实现，**缺一不可**：
///   ① 上面的保留条件 —— 设备**在**枚举结果里、只是读不出数据（HID 层不响应等）。
///      走这条能保住**真实设备信息**（名字、类别、`node_count`）；
///   ② 函数末尾的**反向补建** —— 设备**根本枚举不到**（未连接的耳机、没插的接收器在
///      WMI 里不存在，`Grouper` 中没有它的组）。这条只能用配置里的 alias/fallback 当
///      展示名，`categories` 为空、`node_count` 为 0。
///   只有 ① 会让未连接的设备整条消失；只有 ② 会让「枚举到但读不出」的设备丢掉真实名字。
///   单测里必须用 `node_count` 把两者区分开，否则补建会把 ① 的用例兜成**假绿**。
pub fn group_taskbar_devices(
    devices: &[crate::device::Device],
    audio: &[crate::audio::AudioDevice],
    pinned: &[crate::config::PinnedDevice],
) -> Vec<PhysicalDevice> {
    let mut grouper = Grouper::new();

    // 先入音频端点：音量所需的是端点 id，端点自己也提供一份展示名。
    // ⚠️ 同一容器可能有**多个**输出端点（实测 `084fb1b9-…` 覆盖 3 个），
    // 而 `Grouper` 取先到的那个 ⇒ 这里先把**系统默认**端点排到前面，
    // 让「哪个端点的音量代表这台设备」有确定且符合直觉的答案。
    // `sort_by_key` 是稳定排序 ⇒ 同为默认/非默认时保持原有枚举顺序，结果仍确定。
    let mut ordered: Vec<&crate::audio::AudioDevice> = audio.iter().collect();
    ordered.sort_by_key(|a| !a.is_default);
    for a in ordered {
        let key = audio_endpoint_key(a);
        grouper.add(
            Some(&key.encode()),
            &a.name,
            DevType::Audio,
            None,
            Some(a.id.as_str()),
        );
    }

    // 再入设备列表：电量与类别由它提供。
    // 电量来源按**传输方式**判定 —— 同一容器出现多来源时靠 `BatterySource` 取更可信的那个。
    for d in devices {
        let source = if d.is_bluetooth || d.is_ble {
            BatterySource::Bluetooth
        } else if d.is_wireless_24g {
            BatterySource::Hid24g
        } else {
            BatterySource::Unknown
        };
        let fallback = DeviceKey::Name(core_name(&d.name)).encode();
        grouper.add(
            d.device_key.as_deref().or(Some(fallback.as_str())),
            &d.name,
            d.dt,
            d.battery.map(|b| (b, source)),
            None,
        );
    }

    let mut kept: Vec<PhysicalDevice> = grouper
        .finish()
        .into_iter()
        .filter_map(|mut p| {
            let fallback = DeviceKey::Name(core_name(&p.name)).encode();
            p.pinned = crate::config::matches_pinned_taskbar(pinned, &p.key, Some(&fallback));
            // 固定 ⇒ 强制显示（即使此刻无数据，前端置灰）
            if p.pinned || p.battery.is_some() || p.audio_device_id.is_some() {
                Some(p)
            } else {
                None
            }
        })
        .collect();

    // ── 反向补建：被固定、但本次枚举里**完全没有出现**的设备 ──────────────
    // 未连接的耳机 / 没插的接收器在 WMI 里不存在，上面的保留规则救不到它们。
    // 逐项查 `kept`（含刚补建的）而非查 `pinned` 的其它项 ⇒ 配置里重复的固定项
    // 天然去重：第一条补进 `kept` 后，第二条就会精确命中它。
    for p in pinned {
        let already = kept.iter().any(|d| {
            let fallback = DeviceKey::Name(core_name(&d.name)).encode();
            crate::config::pinned_device_matches(p, &d.key, Some(&fallback))
        });
        if already {
            continue;
        }
        kept.push(PhysicalDevice {
            key: p.key.clone(),
            name: pinned_placeholder_name(p),
            battery: None,
            audio_device_id: None,
            // 占位条目没有参与聚合的节点 ⇒ 无类别、计数为 0
            categories: Vec::new(),
            node_count: 0,
            pinned: true,
        });
    }

    // `Grouper` 的「输出按 key 排序、顺序稳定」契约要在补建之后**重新成立**：
    // 否则占位条目恒排在末尾，位置还随补建顺序变化，UI 会跳。
    // （补建条目的 key 必然与 `kept` 中已有的 key 不同 —— 相同就会在上面的
    //  `already` 判定里精确命中而被跳过，故排序后不会出现重复 key。）
    kept.sort_by(|a, b| a.key.cmp(&b.key));
    kept
}

/// 占位条目的展示名：优先用户自定义名，其次从身份键里剥出可读部分。
///
/// `n:`（名称键）的后半段本身就是可读名字；`c:`（容器 GUID）/ `i:`（实例路径）是机器串，
/// 不含可读信息，只能原样展示 —— 此时 `alias` 是用户唯一能看懂的名字，UI 应引导用户设置。
fn pinned_placeholder_name(p: &crate::config::PinnedDevice) -> String {
    if let Some(a) = p.alias.as_deref() {
        if !a.trim().is_empty() {
            return a.to_string();
        }
    }
    [p.fallback.as_deref(), Some(p.key.as_str())]
        .into_iter()
        .flatten()
        .find_map(|s| {
            s.strip_prefix("n:")
                .map(|r| r.trim())
                .filter(|r| !r.is_empty())
        })
        .map(|s| s.to_string())
        .unwrap_or_else(|| p.key.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_guid_strips_braces_and_case() {
        // 两侧取材形式不同是常态，归一化必须把它们压到同一形态
        assert_eq!(
            normalize_guid("{9039AEA7-9C07-52F2-A4EF-0F5296D6D7D2}"),
            "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2"
        );
        assert_eq!(
            normalize_guid("  9039aea7-9c07-52f2-a4ef-0f5296d6d7d2  "),
            "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2"
        );
        // 两侧归一化后必须相等 —— 这正是上一轮假阴性的直接回归判据
        assert_eq!(
            normalize_guid("{9039AEA7-9C07-52F2-A4EF-0F5296D6D7D2}"),
            normalize_guid("9039aea7-9c07-52f2-a4ef-0f5296d6d7d2")
        );
    }

    /// ⭐ **跨路径判据**：两个**独立**的容器生产者必须产出逐字相等的载荷。
    ///
    /// 为什么需要它：`wireless_24g` 的 HID 分域靠**字符串相等**把两侧对上 ——
    ///
    /// ```text
    /// 生产侧 ①  BatteryTarget.key = "c:" + device_key(Some(container_raw)).encode()
    ///            （container_raw 来自 wmi_query 侧对 PnP 节点的查询）
    /// 生产侧 ②  container_of_instance(hidapi 路径映射出的实例路径)
    ///            （来自 filter_by_scope 的解析器）
    /// ```
    ///
    /// 两侧**各自**走一条归一化/格式化代码路径。若其中一条被改动而另一条没跟上，
    /// `==` 会**恒不成立** ⇒ `filter_by_scope` 恒返回空 ⇒ `enumerate_paths` 恒返回 `Err`
    /// ⇒ **所有 2.4G 电量查询全部失败**（静默、无编译错误）。
    /// 而 `hid_link` 的分域单测用的是**假解析器**，天然覆盖不到这条不变式。
    ///
    /// 本用例把两条路径放在同一个容器上对撞：
    /// ① `format_guid_bytes`（字节 → 字符串，`container_of_instance` 的取值来源）
    /// ② `device_key` → `DeviceKey::encode`（原始字符串 → 键载荷）
    ///
    /// **可证伪**：把 `format_guid_bytes` 的 `{:02x}` 改成 `{:02X}`（或去掉
    /// `device_key` 路径上的 `usable_container` 归一化）本用例即转红。
    ///
    /// ⚠️ **本用例证明不了什么**：它不证明 `CM_Get_DevNode_PropertyW` 真的返回这 16 字节
    /// （那需要实机）。实机侧由只读探针覆盖：全部**在场**设备 245 条上 200 轮
    /// = **49000/49000 次读到容器（100%）**；在**在场 HID** 29 条上 200 轮
    /// = **5800/5800（100%）**。
    /// （⚠️ 245 是「全部在场设备」，**不是** HID 数 —— HID 在场是 29。）
    #[test]
    fn both_container_producers_agree_byte_for_byte() {
        // 取本机真实容器 `0d85362f-9ba5-11f1-b7f2-105fadd8248b` 的**属性缓冲区字节**
        // （前 3 字段小端：Data1 4B / Data2 2B / Data3 2B，Data4 8B 原序）
        let bytes: [u8; 16] = [
            0x2f, 0x36, 0x85, 0x0d, // Data1 = 0d85362f
            0xa5, 0x9b, // Data2 = 9ba5
            0xf1, 0x11, // Data3 = 11f1
            0xb7, 0xf2, 0x10, 0x5f, 0xad, 0xd8, 0x24, 0x8b, // Data4
        ];

        // 生产侧 ②：字节 → 小写无花括号字符串
        let from_bytes = format_guid_bytes(&bytes);
        assert_eq!(from_bytes, "0d85362f-9ba5-11f1-b7f2-105fadd8248b");

        // 生产侧 ①：任意形式的原始字符串 → 键载荷
        // 三种形态都要收敛到同一个载荷（含「带花括号大写」这一注册表常见形态）
        for raw in [
            "{0D85362F-9BA5-11F1-B7F2-105FADD8248B}",
            "0d85362f-9ba5-11f1-b7f2-105fadd8248b",
            "  {0d85362f-9ba5-11f1-b7f2-105fadd8248b}  ",
        ] {
            let key = device_key(Some(raw), Some("HID\\x\\y"), Some("某设备"))
                .expect("有容器时必走 Container 分支")
                .encode();
            assert_eq!(
                key,
                format!("c:{from_bytes}"),
                "容器载荷必须与 container_of_instance 侧逐字相等（raw={raw}）—— \
                 不等则 HID 分域恒不匹配、2.4G 电量全部查不出来"
            );
        }

        // 反控：占位容器必须**不**产出 `c:` 载荷（否则分域会拿占位容器去比，
        // 本机多个互不相关的虚拟音频设备共享它 ⇒ 错误合并）
        let placeholder = device_key(
            Some("{00000000-0000-0000-ffff-ffffffffffff}"),
            Some("HID\\x\\y"),
            Some("某设备"),
        )
        .expect("容器不可用时应降级到实例，而不是丢弃");
        assert_eq!(
            placeholder.encode(),
            "i:hid\\x\\y",
            "占位容器必须降级到实例键（且实例载荷已转小写）"
        );
    }

    #[test]
    fn null_containers_detected_regardless_of_form() {
        assert!(is_null_container("{00000000-0000-0000-ffff-ffffffffffff}"));
        assert!(is_null_container("00000000-0000-0000-FFFF-FFFFFFFFFFFF"));
        assert!(is_null_container("{00000000-0000-0000-0000-000000000000}"));
        // 真实容器绝不能被误判
        assert!(!is_null_container("{9039aea7-9c07-52f2-a4ef-0f5296d6d7d2}"));
    }

    #[test]
    fn usable_container_rejects_missing_empty_and_placeholder() {
        assert_eq!(usable_container(None), None);
        assert_eq!(usable_container(Some("")), None);
        assert_eq!(usable_container(Some("   ")), None);
        assert_eq!(usable_container(Some("{}")), None);
        assert_eq!(
            usable_container(Some("{00000000-0000-0000-ffff-ffffffffffff}")),
            None
        );
        assert_eq!(
            usable_container(Some("{82434CF9-6C86-5716-86B4-116C85D16B28}")),
            Some("82434cf9-6c86-5716-86b4-116c85d16b28".to_string())
        );
    }

    #[test]
    fn device_key_falls_back_in_order() {
        // 1) 有真实容器 ⇒ Container
        assert_eq!(
            device_key(
                Some("{9039aea7-9c07-52f2-a4ef-0f5296d6d7d2}"),
                Some("SWD\\MMDEVAPI\\{x}"),
                Some("耳机")
            ),
            Some(DeviceKey::Container(
                "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2".to_string()
            ))
        );
        // 2) 容器是占位 ⇒ 降到 Instance（这条正是「虚拟音频设备」的路径）
        assert_eq!(
            device_key(
                Some("{00000000-0000-0000-ffff-ffffffffffff}"),
                Some("SWD\\MMDEVAPI\\{c5c40f0a}"),
                Some("扬声器")
            ),
            Some(DeviceKey::Instance("SWD\\MMDEVAPI\\{c5c40f0a}".to_string()))
        );
        // 3) 无容器无实例 ⇒ 降到 Name
        assert_eq!(
            device_key(None, None, Some("某个设备")),
            Some(DeviceKey::Name("某个设备".to_string()))
        );
        // 4) 全空 ⇒ None（调用方应丢弃该节点，而不是造一个空键）
        assert_eq!(device_key(None, None, None), None);
        assert_eq!(device_key(Some(""), Some("  "), Some("")), None);
    }

    /// 同一台物理设备的**多个 HID 集合**必须归为**同一个**身份键。
    ///
    /// 2.4G 接收器普遍是复合设备：一台设备暴露 N 个 HID 集合
    /// （`…&MI_01&Col01` / `&Col02` …，实例段也各不相同），
    /// 而 `device_data::is_wireless_24g` 只看 (VID, PID) ⇒ **每个集合**都会被判为
    /// 2.4G、各推一个电量查询目标。若这些目标拿到不同身份键，
    /// 「一台设备」就会被当成「多台设备」，各自查询、各占一条缓存。
    ///
    /// 实机依据（只读探针，本机 13 个多集合容器）：同一容器的 HID 集合数
    /// 为 2~16 不等，其中 `25A7:FA70` 两台各 **9 个集合**、`1532:0094` 一台 **16 个**，
    /// 且**无任何一个容器跨容器**（0 反例）⇒ 容器优先路径下键必然相同。
    ///
    /// 可证伪：把 `device_key` 改成容器不参与（只按实例路径）即转红。
    #[test]
    fn same_container_hid_collections_share_one_identity_key() {
        const C: &str = "{40e11c06-72bd-5b38-9bd2-0e15079b3b45}";
        let a = device_key(
            Some(C),
            Some("HID\\VID_1532&PID_0094&MI_01&COL01\\8&b16f3a&0&0000"),
            None,
        )
        .unwrap()
        .encode();
        let b = device_key(
            Some(C),
            Some("HID\\VID_1532&PID_0094&MI_01&COL02\\8&b16f3a&0&0001"),
            None,
        )
        .unwrap()
        .encode();
        assert_eq!(a, b, "同一容器的不同 HID 集合必须归为同一台设备");
        assert_eq!(a, "c:40e11c06-72bd-5b38-9bd2-0e15079b3b45");

        // 反控 1：容器缺失时降级到实例路径 ⇒ 必然逐集合拆开。
        // 这是降级路径的**已知代价**（方向是「拆细」而非「串号」）：
        // 不同物理设备的实例路径必然不同，故降级不会把两台设备混为一谈。
        let ia = device_key(
            None,
            Some("HID\\VID_1532&PID_0094&MI_01&COL01\\8&b16f3a&0&0000"),
            None,
        )
        .unwrap()
        .encode();
        let ib = device_key(
            None,
            Some("HID\\VID_1532&PID_0094&MI_01&COL02\\8&b16f3a&0&0001"),
            None,
        )
        .unwrap()
        .encode();
        assert_ne!(ia, ib, "无容器时逐集合拆分 —— 降级路径的固有代价");
        assert!(ia.starts_with("i:") && ib.starts_with("i:"));

        // 反控 2：不同容器（= 不同物理设备）绝不可合并 —— 这正是要修掉的串号缺陷
        let other = device_key(
            Some("{929f52bc-3b4e-11f1-b794-105fadd8248e}"),
            Some("HID\\VID_25A7&PID_FA70&MI_01&COL01\\9&f0862ec&0&0000"),
            None,
        )
        .unwrap()
        .encode();
        assert_ne!(a, other, "不同容器的设备必须分开");
    }

    #[test]
    fn encoded_key_prefixes_are_distinct() {
        let c = DeviceKey::Container("abc".to_string()).encode();
        let i = DeviceKey::Instance("abc".to_string()).encode();
        let n = DeviceKey::Name("abc".to_string()).encode();
        assert_eq!(c, "c:abc");
        assert_eq!(i, "i:abc");
        assert_eq!(n, "n:abc");
        // 同名字符串在三级之间不得碰撞
        assert_ne!(c, i);
        assert_ne!(i, n);
        // 实例路径大小写不敏感 ⇒ 编码时统一小写，避免同一设备产生两个键
        assert_eq!(
            DeviceKey::Instance("USB\\VID_046D&PID_C092".to_string()).encode(),
            DeviceKey::Instance("usb\\vid_046d&pid_c092".to_string()).encode()
        );
    }

    #[test]
    fn format_guid_bytes_is_little_endian_for_first_three_fields() {
        // 9039aea7-9c07-52f2-a4ef-0f5296d6d7d2 的字节序表示：
        // Data1=0x9039aea7 → a7 ae 39 90；Data2=0x9c07 → 07 9c；Data3=0x52f2 → f2 52
        let bytes = [
            0xa7, 0xae, 0x39, 0x90, 0x07, 0x9c, 0xf2, 0x52, 0xa4, 0xef, 0x0f, 0x52, 0x96, 0xd6,
            0xd7, 0xd2,
        ];
        assert_eq!(
            format_guid_bytes(&bytes),
            "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2"
        );
        // 归一化后与「带花括号大写」形式相等 —— 端到端一致性
        assert_eq!(
            normalize_guid("{9039AEA7-9C07-52F2-A4EF-0F5296D6D7D2}"),
            format_guid_bytes(&bytes)
        );
    }

    /// 下面的实例路径全部是**本机实测原文**（`.workbuddy-ai/scratch/probe_bt_container_map.py`）。
    #[test]
    fn mac_extracted_from_both_bluetooth_instance_shapes() {
        // 形态一：设备节点（经典蓝牙）
        assert_eq!(
            mac_from_bluetooth_instance(
                r"BTHENUM\Dev_5088112E80E8\8&1d39e19e&0&BluetoothDevice_5088112E80E8"
            ),
            Some("5088112E80E8".to_string())
        );
        // 形态一：设备节点（BLE，全小写）
        assert_eq!(
            mac_from_bluetooth_instance(r"BTHLE\Dev_a4c13830b225\8&7ab9f01&0&a4c13830b225"),
            Some("A4C13830B225".to_string())
        );
        // 形态二：服务节点
        assert_eq!(
            mac_from_bluetooth_instance(
                r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&00021d6b_PID&0246\8&1d39e19e&0&5088112E80E8_C00000000"
            ),
            Some("5088112E80E8".to_string())
        );
    }

    /// 回归判据：**「第一个 12 位十六进制串」这条错误取法必须被证伪**。
    /// 服务节点形态下它会命中 A2DP 服务 GUID 里的 `00805F9B34FB`，本机 3/3 全错。
    #[test]
    fn naive_first_hex12_would_be_wrong() {
        let svc = r"BTHENUM\{0000110b-0000-1000-8000-00805f9b34fb}_VID&00021d6b_PID&0246\8&1d39e19e&0&5088112E80E8_C00000000";
        // 错误取法（模拟「找到第一个连续 12 位十六进制」）：命中服务 GUID 尾部
        let upper = svc.to_ascii_uppercase();
        let naive = upper
            .as_bytes()
            .windows(12)
            .find(|w| w.iter().all(|b| b.is_ascii_hexdigit()))
            .map(|w| String::from_utf8_lossy(w).to_string());
        assert_eq!(naive.as_deref(), Some("00805F9B34FB"));
        // 正确取法给出真正的 MAC，且**不等于**上面那个值
        let correct = mac_from_bluetooth_instance(svc);
        assert_eq!(correct.as_deref(), Some("5088112E80E8"));
        assert_ne!(naive, correct);
    }

    #[test]
    fn mac_extraction_rejects_non_bluetooth_instances() {
        // 普通 USB / SWD 实例不得被误判为蓝牙 MAC
        assert_eq!(
            mac_from_bluetooth_instance(r"USB\VID_046D&PID_C092\5&1A2B3C4D&0&1"),
            None
        );
        assert_eq!(
            mac_from_bluetooth_instance(
                r"SWD\MMDEVAPI\{0.0.0.00000000}.{7e42f2f4-6a35-4f5f-9de8-f8c57b82e68a}"
            ),
            None
        );
        // 含 `DEV_` 但后面不是 12 位十六进制 ⇒ 不误判
        assert_eq!(mac_from_bluetooth_instance(r"ROOT\DEV_UNKNOWN\0000"), None);
    }

    // ── 分组（批 5）────────────────────────────────────────

    fn dev(
        name: &str,
        key: Option<&str>,
        battery: Option<i32>,
        bt: bool,
        g24: bool,
    ) -> crate::device::Device {
        crate::device::Device {
            name: name.to_string(),
            dt: DevType::Audio,
            status: "已连接".to_string(),
            battery,
            device_id: None,
            device_key: key.map(|k| k.to_string()),
            is_bluetooth: bt,
            is_wireless_24g: g24,
            is_ble: false,
        }
    }

    fn audio(name: &str, id: &str, container: Option<&str>) -> crate::audio::AudioDevice {
        crate::audio::AudioDevice {
            id: id.to_string(),
            name: name.to_string(),
            volume: 0.5,
            is_muted: false,
            is_default: false,
            container_id: container.map(|c| c.to_string()),
        }
    }

    /// 核心判据：**蓝牙设备行 + 音频端点行 + 端点本身，必须并成一台**，
    /// 且电量和音量出现在同一条上 —— 这正是任务栏要的东西。
    #[test]
    fn same_container_merges_battery_and_volume_into_one_device() {
        const CID: &str = "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2";
        let key = format!("c:{CID}");
        let devices = vec![
            // 蓝牙行：只有电量，名字是设备名
            dev("小爱音箱-9205", Some(&key), Some(80), true, false),
            // PnP 的音频端点行：名字带括号设备名，无电量
            dev("耳机 (小爱音箱-9205)", Some(&key), None, false, false),
        ];
        let audios = vec![audio("耳机 (小爱音箱-9205)", "{0.0.0.0}.{abc}", Some(CID))];

        let out = group_taskbar_devices(&devices, &audios, &[]);
        assert_eq!(out.len(), 1, "同容器必须并成一台，实际 {out:?}");
        let d = &out[0];
        assert_eq!(d.key, key);
        assert_eq!(d.battery, Some(80), "电量应保留");
        assert_eq!(d.audio_device_id.as_deref(), Some("{0.0.0.0}.{abc}"));
        assert_eq!(d.node_count, 3);
        // 展示名应取「括号内设备名」，而不是毫无区分度的「耳机」
        assert_eq!(d.name, "小爱音箱-9205");
    }

    /// 占位容器不进分组：两个虚拟音频设备各自独立，且因「无电量无音量」被丢弃。
    #[test]
    fn placeholder_container_devices_never_merge_and_are_dropped() {
        let devices = vec![
            dev("扬声器 (网易虚拟音频设备)", None, None, false, false),
            dev(
                "扬声器 (Steam Streaming Speakers)",
                None,
                None,
                false,
                false,
            ),
        ];
        // 两个虚拟端点都拿不到容器 ⇒ 各自退回自己的实例路径，不会共用一个键
        let audios = vec![
            audio("扬声器 (网易虚拟音频设备)", "{0.0.0.0}.{c5c40f0a}", None),
            audio("扬声器 (Steam Streaming)", "{0.0.0.0}.{c5fc3377}", None),
        ];
        let out = group_taskbar_devices(&devices, &audios, &[]);
        assert_eq!(out.len(), 2, "两个虚拟端点不得被合并成一台：{out:?}");
        // 键必须互不相同（合并就会相等）
        assert_ne!(out[0].key, out[1].key);
        // 两者都没有电量；音量则各自有 ⇒ 不应被丢弃
        assert!(out.iter().all(|d| d.battery.is_none()));
        assert!(out.iter().all(|d| d.audio_device_id.is_some()));
    }

    #[test]
    fn entries_without_battery_and_without_volume_are_dropped() {
        // 一个既无电量、又无音频端点的容器（例如被过滤剩下的空壳）
        let devices = vec![dev("某个空壳设备", Some("c:deadbeef"), None, false, false)];
        let out = group_taskbar_devices(&devices, &[], &[]);
        assert!(out.is_empty(), "既无电量又无音量的条目不该投放：{out:?}");
    }

    #[test]
    fn battery_source_priority_prefers_bluetooth_over_24g() {
        const CID: &str = "aaaa1111-2222-3333-4444-555566667777";
        let key = format!("c:{CID}");
        let mut g = Grouper::new();
        // 先加 2.4G 来源，再加蓝牙来源 ⇒ 应取蓝牙（数值更大）
        g.add(
            Some(&key),
            "设备",
            DevType::Usb,
            Some((40, BatterySource::Hid24g)),
            None,
        );
        g.add(
            Some(&key),
            "设备",
            DevType::Usb,
            Some((70, BatterySource::Bluetooth)),
            None,
        );
        let out = g.finish();
        assert_eq!(out[0].battery, Some(70), "应取来源更可信的那个");
        // 反向顺序也应得到同一结果（不依赖枚举顺序）
        let mut g2 = Grouper::new();
        g2.add(
            Some(&key),
            "设备",
            DevType::Usb,
            Some((70, BatterySource::Bluetooth)),
            None,
        );
        g2.add(
            Some(&key),
            "设备",
            DevType::Usb,
            Some((40, BatterySource::Hid24g)),
            None,
        );
        assert_eq!(g2.finish()[0].battery, Some(70));
    }

    #[test]
    fn nodes_without_key_are_dropped_and_output_is_order_independent() {
        // 无身份键的节点直接丢弃
        let mut g = Grouper::new();
        g.add(
            None,
            "没有身份的节点",
            DevType::Other,
            Some((50, BatterySource::Unknown)),
            None,
        );
        assert!(g.finish().is_empty());

        // 输出顺序稳定：与加入顺序无关（BTreeMap 排序）
        let mut a = Grouper::new();
        a.add(
            Some("c:bbb"),
            "B",
            DevType::Usb,
            Some((1, BatterySource::Unknown)),
            None,
        );
        a.add(
            Some("c:aaa"),
            "A",
            DevType::Usb,
            Some((2, BatterySource::Unknown)),
            None,
        );
        let mut b = Grouper::new();
        b.add(
            Some("c:aaa"),
            "A",
            DevType::Usb,
            Some((2, BatterySource::Unknown)),
            None,
        );
        b.add(
            Some("c:bbb"),
            "B",
            DevType::Usb,
            Some((1, BatterySource::Unknown)),
            None,
        );
        let ka: Vec<String> = a.finish().into_iter().map(|d| d.key).collect();
        let kb: Vec<String> = b.finish().into_iter().map(|d| d.key).collect();
        assert_eq!(ka, kb);
        assert_eq!(ka, vec!["c:aaa".to_string(), "c:bbb".to_string()]);
    }

    #[test]
    fn display_name_prefers_paren_device_name_over_bare_desc() {
        assert_eq!(
            pick_display_name(&["耳机".to_string(), "耳机 (小爱音箱-9205)".to_string()]),
            "小爱音箱-9205"
        );
        // 输入顺序无关
        assert_eq!(
            pick_display_name(&["耳机 (小爱音箱-9205)".to_string(), "耳机".to_string()]),
            "小爱音箱-9205"
        );
        // 全部为空 ⇒ 兜底
        assert_eq!(pick_display_name(&[]), "未知设备");
        assert_eq!(pick_display_name(&["   ".to_string()]), "未知设备");
    }

    #[test]
    fn pinned_flag_matches_by_key_then_fallback() {
        const CID: &str = "9039aea7-9c07-52f2-a4ef-0f5296d6d7d2";
        let pinned = vec![
            // 按容器键精确固定
            crate::config::PinnedDevice {
                key: format!("c:{CID}"),
                fallback: None,
                alias: None,
            },
            // 容器变了（换机 / 重装驱动）⇒ 靠**名称键**兜底认出是同一台
            crate::config::PinnedDevice {
                key: "c:已失效的旧容器".to_string(),
                fallback: Some("n:罗技接收器".to_string()),
                alias: Some("我的接收器".to_string()),
            },
        ];

        let devices = vec![
            dev(
                "小爱音箱-9205",
                Some(&format!("c:{CID}")),
                Some(80),
                true,
                false,
            ),
            // 容器是「换过的新容器」，与 pinned 的 key 不符 ⇒ 只能靠名称兜底
            dev("罗技接收器", Some("c:换过的新容器"), Some(90), false, true),
            // 反控：key 与 fallback 都不命中 ⇒ 必须**不**被判定为已固定
            dev("无关设备", Some("c:unrelated"), Some(50), false, false),
        ];

        let out = group_taskbar_devices(&devices, &[], &pinned);
        assert_eq!(out.len(), 3, "三台互不同容器，不得合并：{out:?}");

        let pinned_keys: Vec<&str> = out
            .iter()
            .filter(|d| d.pinned)
            .map(|d| d.key.as_str())
            .collect();
        assert_eq!(
            pinned_keys,
            vec![format!("c:{CID}"), "c:换过的新容器".to_string()],
            "第二台应靠名称键兜底被认出"
        );
        // 反控必须成立，否则说明「已固定」判据恒真
        assert!(
            out.iter().any(|d| d.key == "c:unrelated" && !d.pinned),
            "无关设备不应被判定为已固定"
        );
    }

    /// 被固定但**此刻读不出数据**的设备必须保留（前端置灰），不得被「至少能显示一项信息」
    /// 的丢弃规则吃掉 —— 否则 pin 形同虚设。
    ///
    /// 判据可证伪：把保留条件改回 `p.battery.is_some() || p.audio_device_id.is_some()`
    /// （即去掉 `p.pinned ||`），本用例必转红。
    #[test]
    fn pinned_device_without_data_is_kept_for_gray_out() {
        let devices = vec![
            // 被固定，但电量读不出（如 HID 层不响应）、也没有音频端点
            dev("罗技接收器", Some("c:pinned-nodata"), None, false, true),
            // 反控：同样无数据、但**未**被固定 ⇒ 必须照旧丢弃
            dev("无关设备", Some("c:unrelated"), None, false, false),
        ];
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:pinned-nodata".to_string(),
            fallback: None,
            alias: None,
        }];

        let out = group_taskbar_devices(&devices, &[], &pinned);
        let keys: Vec<&str> = out.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(keys, vec!["c:pinned-nodata"], "只应保留被固定那台：{out:?}");
        assert!(out[0].pinned, "固定标记必须为真");
        assert!(
            out[0].battery.is_none() && out[0].audio_device_id.is_none(),
            "前置：这台设备确实读不出数据"
        );
        // ⭐ 这两条断言把「保留条件」与「反向补建」**区分开** —— 否则本用例是**假绿**：
        //    设备被 filter 掉后，末尾的补建循环会照着 `pinned` 再造一条，键与 `pinned`
        //    标记照样对得上，`battery` 也照样是 `None`，三条断言全满足。
        //    但补建条目丢失了**真实设备信息**（名字退化成配置里的 alias/fallback、
        //    类别为空、`node_count` 为 0）。设备**在**枚举结果里时，必须走保留分支。
        //    判据可证伪：去掉保留条件里的 `p.pinned ||`，下面两条必转红。
        assert_eq!(out[0].node_count, 1, "必须走保留分支而非补建：{out:?}");
        assert_eq!(out[0].categories.len(), 1, "类别应来自真实设备行");
    }

    /// 被固定、但**本次枚举里根本没出现**的设备（未连接的耳机 / 没插的接收器）必须
    /// **反向补建**占位条目 —— 只把固定判定提前救不了这一种：`Grouper` 里没有它的组，
    /// `finish()` 自然不产出该条目。
    ///
    /// 判据可证伪：删掉 `group_taskbar_devices` 末尾的 `for p in pinned` 补建循环，
    /// 本用例必转红。
    #[test]
    fn pinned_device_absent_from_enumeration_is_synthesized() {
        let pinned = vec![crate::config::PinnedDevice {
            // 容器键是机器串，不含可读信息 ⇒ 展示名只能从 fallback 的名称键里剥
            key: "c:9039aea7-9c07-52f2-a4ef-0f5296d6d7d2".to_string(),
            fallback: Some("n:我的耳机".to_string()),
            alias: None,
        }];
        // 枚举结果与它毫无关系
        let devices = vec![dev("别的东西", Some("c:other"), Some(50), false, false)];

        let out = group_taskbar_devices(&devices, &[], &pinned);
        let ph = out
            .iter()
            .find(|d| d.key == "c:9039aea7-9c07-52f2-a4ef-0f5296d6d7d2")
            .expect("被固定但枚举不到的设备必须补建占位条目");
        assert!(ph.pinned, "补建条目必须标记为已固定");
        assert_eq!(ph.name, "我的耳机", "展示名应从 fallback 的名称键剥出");
        assert!(ph.battery.is_none() && ph.audio_device_id.is_none());
        assert_eq!(ph.node_count, 0, "占位条目没有节点参与聚合");
    }

    /// `alias`（用户自定义名）优先于从身份键里剥出的名字；都拿不到可读名时退回原 key。
    ///
    /// 判据可证伪：删掉 `pinned_placeholder_name` 里的 `alias` 分支，本用例必转红。
    #[test]
    fn pinned_placeholder_prefers_alias_over_key_derived_name() {
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:some-guid".to_string(),
            fallback: Some("n:设备原名".to_string()),
            alias: Some("我的接收器".to_string()),
        }];
        let out = group_taskbar_devices(&[], &[], &pinned);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].name, "我的接收器");

        // 展示名退到键派生时，`c:` 机器串没有可读部分 ⇒ 原样返回 key（绝不能是空串）
        let no_name = vec![crate::config::PinnedDevice {
            key: "c:some-guid".to_string(),
            fallback: None,
            alias: None,
        }];
        let out = group_taskbar_devices(&[], &[], &no_name);
        assert_eq!(out[0].name, "c:some-guid", "无可读名字时退回原 key");
    }

    /// 补建不得与「已有实际数据的同一台设备」重复；且补建之后**仍按 key 排序**
    /// （`Grouper` 的「顺序稳定」契约不能因为补建而失效）。
    ///
    /// 判据可证伪：删掉末尾的 `kept.sort_by(..)`，本用例的排序断言必转红
    /// （占位条目会恒排在末尾，即 `["c:aaa", "c:zzz", "c:bbb"]`）。
    #[test]
    fn pinned_placeholder_does_not_duplicate_present_device_and_keeps_order() {
        let devices = vec![
            dev("罗技接收器", Some("c:aaa"), Some(90), false, true),
            dev("别的东西", Some("c:zzz"), Some(50), false, false),
        ];
        let pinned = vec![
            crate::config::PinnedDevice {
                key: "c:aaa".to_string(), // 已有实际数据 ⇒ 不得重复补建
                fallback: None,
                alias: None,
            },
            crate::config::PinnedDevice {
                key: "c:bbb".to_string(), // 枚举不到 ⇒ 补建
                fallback: None,
                alias: None,
            },
        ];

        let out = group_taskbar_devices(&devices, &[], &pinned);
        let keys: Vec<&str> = out.iter().map(|d| d.key.as_str()).collect();
        assert_eq!(
            keys,
            vec!["c:aaa", "c:bbb", "c:zzz"],
            "不得重复，且必须按 key 排序：{out:?}"
        );
        assert_eq!(
            out.iter().filter(|d| d.pinned).count(),
            2,
            "两项固定都应命中（一项实际、一项占位）"
        );
    }

    /// 同一容器有**多个**输出端点时，音量必须落在**系统默认**那个端点上。
    ///
    /// 否则「这台设备的音量」会取决于端点的枚举顺序 —— 用户拖动音量条时调的是另一个
    /// 端点，表现为「调了没反应」。实机 `084fb1b9-…`（Mijia Glasses Lite）正是这种容器。
    ///
    /// 判据可证伪：删掉 `group_taskbar_devices` 里的
    /// `ordered.sort_by_key(|a| !a.is_default)`，本用例必转红（会选中先枚举的非默认端点）。
    #[test]
    fn default_endpoint_wins_when_container_has_multiple_endpoints() {
        const CID: &str = "084fb1b9-ff11-5ca5-ba77-242db9091204";
        let key = format!("c:{CID}");
        let non_default = audio("扬声器 (Mijia Glasses Lite)", "{0.0.0.0}.{aaa}", Some(CID));
        let mut default = audio("耳机 (Mijia Glasses Lite)", "{0.0.0.0}.{bbb}", Some(CID));
        default.is_default = true;
        assert!(!non_default.is_default, "前置：第一个端点必须是非默认的");
        // 故意把**非默认**端点排在前面：不排序就会选中它
        let audios = vec![non_default, default];

        let devices = vec![dev("Mijia Glasses Lite", Some(&key), Some(90), true, false)];
        let out = group_taskbar_devices(&devices, &audios, &[]);
        assert_eq!(out.len(), 1, "同容器必须并成一台：{out:?}");
        assert_eq!(
            out[0].audio_device_id.as_deref(),
            Some("{0.0.0.0}.{bbb}"),
            "音量端点应取系统默认的那个，而不是先枚举到的那个"
        );
        // 展示名不因排序而变：两份名字的 core_name 相同，取到的仍是设备名
        assert_eq!(out[0].name, "Mijia Glasses Lite");
    }

    /// `devnode_from_hidapi_path`：三条**实机实测**样本（均取自本机 hidapi 输出，
    /// 且映射结果都能在 `CM_Get_Device_ID_ListW` 的 HID 清单里找到）。
    ///
    /// 覆盖三个必须做对的点：`ColNN` 原样保留（不做大写）、
    /// 尾部 `\KBD` 后缀必须丢掉、`GVInput` 这类无 VID/PID 的设备 ID 段也要能过。
    #[test]
    fn hidapi_path_maps_to_devnode_instance() {
        // ① 复合接收器的集合接口：hidapi 给 `Col07`，devnode 是 `COL07`（大小写不敏感）
        assert_eq!(
            devnode_from_hidapi_path(
                r"\\?\HID#VID_1532&PID_0094&MI_01&Col07#8&b16f3a&0&0006#{4d1e55b2-f16f-11cf-88cb-001111000030}"
            )
            .as_deref(),
            Some(r"HID\VID_1532&PID_0094&MI_01&Col07\8&b16f3a&0&0006")
        );
        // ② 尾部 `\KBD` 后缀（接口类 GUID 之后还有内容）必须被截掉
        assert_eq!(
            devnode_from_hidapi_path(
                r"\\?\HID#VID_1532&PID_0094&MI_02#8&2488acfc&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}\KBD"
            )
            .as_deref(),
            Some(r"HID\VID_1532&PID_0094&MI_02\8&2488acfc&0&0000")
        );
        // ③ 无 VID/PID 的软件 HID（本机 GVInput 落在占位容器上）
        assert_eq!(
            devnode_from_hidapi_path(
                r"\\?\HID#GVInput&Col03#1&2d595ca7&0&0002#{4d1e55b2-f16f-11cf-88cb-001111000030}\KBD"
            )
            .as_deref(),
            Some(r"HID\GVInput&Col03\1&2d595ca7&0&0002")
        );
    }

    /// ⛔ 关键判据：**必须从最后一个 `#{` 截断**。
    ///
    /// 蓝牙 HID 的设备 ID 段本身以 `{00001812-…}` 开头（`HID#{GUID}_Dev_…`）。
    /// 若按**第一个** `#{` 切，设备 ID 段会被整段丢掉，映射结果变成 `HID\a&…`，
    /// 定位必然失败 ⇒ 该设备的 HID 集合全部拿不到容器、分域失效。
    ///
    /// 可证伪：把 `rfind("#{")` 改成 `find("#{")`，本用例必转红。
    ///
    /// ⚠️ 本机当前**未接**这类设备 ⇒ 期望值取自只读注册表探针
    /// （`BaseContainers` 的成员实例路径），属**结构外推**，未经 `CM_Locate_DevNodeW` 实测。
    #[test]
    fn hidapi_path_keeps_guid_prefixed_device_id_segment() {
        let out = devnode_from_hidapi_path(
            r"\\?\HID#{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&021532_PID&0095_REV&0001_c6947e50a677&Col01#a&1ed5719f&0&0000#{4d1e55b2-f16f-11cf-88cb-001111000030}",
        );
        assert_eq!(
            out.as_deref(),
            Some(
                r"HID\{00001812-0000-1000-8000-00805f9b34fb}_Dev_VID&021532_PID&0095_REV&0001_c6947e50a677&Col01\a&1ed5719f&0&0000"
            ),
            "设备 ID 段（含 {{GUID}} 前缀）必须逐字保留"
        );
    }

    /// 反控：结构不成立时必须返回 `None`，不能凭猜测造一个路径出来
    /// —— 否则会拿一个不存在的实例去 `CM_Locate_DevNodeW`，白跑还掩盖真因。
    #[test]
    fn hidapi_path_rejects_malformed_input() {
        assert_eq!(devnode_from_hidapi_path(""), None);
        assert_eq!(
            devnode_from_hidapi_path(r"\\?\HID#VID_1532"),
            None,
            "段数不足"
        );
        assert_eq!(
            devnode_from_hidapi_path(r"\\?\HID##8&b16f3a&0&0000"),
            None,
            "设备 ID 段为空"
        );
        // 无 `\\?\` 前缀也要能处理（hidapi 之外来源可能不带）
        assert_eq!(
            devnode_from_hidapi_path("HID#VID_046D&PID_C092#7&1a2b3c4d&0&0000").as_deref(),
            Some(r"HID\VID_046D&PID_C092\7&1a2b3c4d&0&0000")
        );
    }
}

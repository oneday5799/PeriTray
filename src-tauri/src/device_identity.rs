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
use std::collections::{BTreeMap, BTreeSet, HashMap};
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

/// 任务栏 widget 上给「这台设备」画哪一类图标。
///
/// 判据优先级：有音频端点时按端点前缀；没有音频端点时，只有已识别的 2.4G
/// 鼠标 / 键盘 / 手柄才使用对应图标，其余设备使用默认图标。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum AudioKind {
    /// 无具体分类，或未识别的无音频设备；默认图标同时也是 2.4G 鼠标图标。
    #[default]
    Pointer,
    /// 2.4G 键盘。
    Keyboard,
    /// 2.4G 手柄。
    Gamepad,
    /// 扬声器 / 其它有音频输出但非耳机。
    Speaker,
    /// 耳机。
    Headphones,
}

/// 从**音频端点原始名**归一化出图标类别。
///
/// ⚠️ 判据是「**前缀**匹配」而不是「包含」：Windows 的端点名形如
///   `扬声器 (Realtek(R) Audio)`、`耳机 (WH-1000XM5)`，
///   语义在**开头那两个字**里。用 `contains` 会让「某设备叫『耳机支架』的扬声器」
///   被误判成耳机 ⇒ 只取 ` (` 之前的**前缀**再判等。
///
/// ⚠️ 兼容**本地化**：不同系统语言下前缀可能是英文（`Speakers` / `Headphones`）
///   ⇒ 同时接受中英两种写法，避免英文系统上全部退化成 Speaker。
pub fn audio_kind_from_endpoint_name(name: Option<&str>) -> AudioKind {
    let Some(n) = name else {
        return AudioKind::Pointer; // 无端点 ⇒ 由上层按 2.4G 类型进一步选择
    };
    // 取 ` (` 之前的前缀（与 `dedup::core_name` 的切分口径一致）
    let prefix = match n.find(" (") {
        Some(i) => &n[..i],
        None => n,
    };
    let p = prefix.trim();
    // 耳机：中英两种本地化写法
    if p.starts_with("耳机")
        || p.eq_ignore_ascii_case("headphones")
        || p.eq_ignore_ascii_case("headset")
    {
        AudioKind::Headphones
    } else {
        // 其余一切有端点的情形（含 `扬声器` / `Speakers` / 显示器音频等）都算「喇叭」
        AudioKind::Speaker
    }
}

/// 从无音频端点设备的 2.4G 类型选择任务栏图标。
///
/// 未识别、非 2.4G 或无类型信息一律回退到默认图标；默认图标与 2.4G 鼠标图标
/// 使用同一份资源，这是需求刻意指定的语义。
pub fn audio_kind_from_wireless_kind(kind: Option<crate::device::Wireless24gKind>) -> AudioKind {
    match kind {
        Some(crate::device::Wireless24gKind::Keyboard) => AudioKind::Keyboard,
        Some(crate::device::Wireless24gKind::Gamepad) => AudioKind::Gamepad,
        Some(crate::device::Wireless24gKind::Mouse) | None => AudioKind::Pointer,
    }
}

/// 一台**物理设备** —— 把同一容器下的各功能节点聚合后的结果。
///
/// 这是任务栏信息窗的数据单元：电量来自蓝牙属性 / HID，音量来自该设备的**输出**端点。
///
/// ⚠️ **「置灰占位」条目的判据**：`connected == false` 且 `node_count == 0` 时，
/// 这条是「被用户固定、但此刻枚举不到（未连接/未插）」的反向补建。前端应置灰呈现
/// 而非隐藏；已连接但暂时读不出电量的 Xbox 等设备不能因此被误判为离线。
/// 电量仍须保留三态：`Some(0)` 是合法电量，不能用布尔真假判断。
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
    /// 上面那个端点的音量（`0.0`–`1.0`）；无音频端点时为 `None`。
    ///
    /// ⭐ **随设备一起返回，而不是让前端自己去 `get_audio_devices` 里 join** ——
    /// 否则「用 `audio_device_id` 匹配 `AudioDevice.id`」就成了**未文档化的隐式契约**：
    /// 前端写错（例如改用 `name` 匹配）既不报错也不告警，只会静默显示错误的音量。
    /// 聚合时 `AudioDevice` 本来就在手边，带上它零成本，还省掉一次 IPC。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<f32>,
    /// 该输出端点是否静音；无音频端点时为 `None`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_muted: Option<bool>,
    /// 该输出端点是否为**系统默认**设备；无音频端点时为 `None`
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// 音频端点的**原始名**（如 `扬声器 (Realtek Audio)`、`耳机 (WH-1000XM5)`）；
    /// 无音频端点时为 `None`。
    ///
    /// ⭐ 为什么必须**原样**带上，而不是只带 `audio_device_id`：
    ///   任务栏图标要按「**出现在音量页**（即本字段存在）且原始名是**扬声器**还是**耳机**」
    ///   来决定画哪个图标（见 `AudioKind`）。端点名是 Windows 给的本地化字符串，
    ///   形如 `扬声器 (设备名)` / `耳机 (设备名)` —— **括号前缀**才承载「扬声器/耳机」语义，
    ///   而 `name` 字段已被 `pick_display_name` 换成括号内的物理设备名（`WH-1000XM5`），
    ///   丢失了前缀 ⇒ 必须单独保留原始串。
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_endpoint_name: Option<String>,
    /// 由音频端点或无音频设备的 2.4G 类型归一出的图标类别（任务栏 widget 用）。
    pub audio_kind: AudioKind,
    /// 该容器出现过的设备类别（一个容器可跨多类，实测 USB + HID + SWD）；占位条目为空数组
    pub categories: Vec<DevType>,
    /// 参与聚合的条目数（设备行 + 音频端点行）；占位条目为 0。
    ///
    /// ⚠️ 这**不是** devnode 数：PeriTray 的设备列表在 WMI 层已过滤掉大部分子节点
    /// （`is_generic_hid` 滤 `&COL*`、`is_bt_service` 滤蓝牙服务节点），所以这里通常很小。
    /// 「10 个 devnode 并成 1 台」是 ContainerId 在 devnode 层面的性质，不由此字段体现。
    pub node_count: usize,
    /// 该物理设备是否在线/已连接；用于区分「已连接但暂时读不到电量」与占位条目。
    pub connected: bool,
    /// 是否被用户固定（由 config 决定，`Grouper` 本身不关心）。
    ///
    /// ⚠️ 语义是「**强制显示**」：为真的条目即使此刻无数据也会保留
    /// （见 `group_taskbar_devices` 的保留规则与补建循环）；只有 `connected == false`
    /// 的占位条目由前端置灰。
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
    /// 该容器命中的输出端点（**整条**存下，含音量 / 静音 / 默认标记）。
    ///
    /// 不满足于只存 id：`finish()` 要从它一次派生 `audio_device_id` / `volume` /
    /// `is_muted` / `is_default` 四个字段，而这四个必须**同生同灭** ——
    /// 分开存就可能出现「有 id 却没有音量」的半截状态。
    audio: Option<crate::audio::AudioDevice>,
    wireless_24g_kind: Option<crate::device::Wireless24gKind>,
    connected: bool,
    categories: Vec<DevType>,
    node_count: usize,
}

impl Grouper {
    pub fn new() -> Self {
        Self::default()
    }

    /// 加入一个节点。`key` 为 `None` 的节点**直接丢弃** —— 没有身份的设备不该出现在任务栏。
    ///
    /// `audio` 是该节点命中的输出端点：**音频端点行传 `Some`，设备行传 `None`**。
    /// 参数**个数保持不变**（用整条 `AudioDevice` 顶替原先的 `Option<&str>` id），
    /// 避免把 `add` 推进 `too_many_arguments` —— 本仓 clippy 基线对它是收紧的，
    /// 新增一处即转红。
    pub fn add(
        &mut self,
        key: Option<&str>,
        name: &str,
        dt: DevType,
        battery: Option<(i32, BatterySource)>,
        audio: Option<&crate::audio::AudioDevice>,
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
        // 同一容器可能命中**多个**端点（调用方已把默认端点排到前面）⇒ 取**先到**的那个，
        // 与「哪个端点的音量代表这台设备」的排序约定一致。
        if let Some(a) = audio {
            if g.audio.is_none() {
                g.audio = Some(a.clone());
            }
        }
        if !g.categories.contains(&dt) {
            g.categories.push(dt);
        }
        g.node_count += 1;
    }

    /// 给已加入的设备组补充 2.4G 具体类型，不改变 `add` 的参数契约。
    pub fn set_wireless_24g_kind(
        &mut self,
        key: &str,
        kind: Option<crate::device::Wireless24gKind>,
    ) {
        if let Some(kind) = kind {
            let group = self.acc.entry(key.to_string()).or_default();
            if group.wireless_24g_kind.is_none() {
                group.wireless_24g_kind = Some(kind);
            }
        }
    }

    /// 记录物理设备是否在线；占位条目不会调用此方法，保持 `false`。
    pub fn set_connected(&mut self, key: &str, connected: bool) {
        if connected {
            self.acc.entry(key.to_string()).or_default().connected = true;
        }
    }

    pub fn finish(self) -> Vec<PhysicalDevice> {
        self.acc
            .into_iter()
            .map(|(key, g)| {
                // 四个音频字段从**同一条**端点派生 ⇒ 不可能出现「有 id 没音量」的半截状态
                let audio = g.audio;
                PhysicalDevice {
                    key,
                    name: pick_display_name(&g.names),
                    battery: g.battery.map(|(level, _)| level),
                    audio_device_id: audio.as_ref().map(|a| a.id.clone()),
                    volume: audio.as_ref().map(|a| a.volume),
                    is_muted: audio.as_ref().map(|a| a.is_muted),
                    is_default: audio.as_ref().map(|a| a.is_default),
                    // ⭐ 原始端点名与由其派生的图标类别必须**同生同灭**
                    //    （都来自同一个 `audio`）—— 分开算就可能出现「有名字没类别」的半截状态
                    audio_endpoint_name: audio.as_ref().map(|a| a.name.clone()),
                    audio_kind: match audio.as_ref() {
                        Some(a) => audio_kind_from_endpoint_name(Some(a.name.as_str())),
                        None => audio_kind_from_wireless_kind(g.wireless_24g_kind),
                    },
                    categories: g.categories,
                    node_count: g.node_count,
                    connected: g.connected,
                    pinned: false,
                }
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

/// 音频端点 → 身份键。有**可用**容器用容器；否则退回端点自己的 devnode 实例路径
/// （`SWD\MMDEVAPI\{id}`），这样它仍能与 PnP 侧同一端点的 `Device.device_key` 对齐。
///
/// ⭐ **虚拟音频设备走的正是降级这条路** —— 它们落在**占位容器**上，
/// 而 `container_of_audio_endpoint` 内部已过 `usable_container` ⇒ 传进来时
/// `container_id` **就是 `None`**（不是「占位容器字符串」）。此处再兜一道底：
/// 万一上游哪天把占位容器原样填进来，这里也必须降级，否则占位容器上的多条无关设备
/// （实测网易×2 + Steam×1 共享 `{00000000-0000-0000-FFFF-FFFFFFFFFFFF}`）
/// 会被 `Grouper` **并成一条** —— 比现状更糟。
///
/// ⛔ **此处必须自己复核、不得只信任调用方**（与 `normalize_encoded_key` 同一条纪律）：
/// 本函数把「容器串已归一化/已排除占位」的责任收回来自己承担，
/// 代价是一次 `usable_container` 调用，换来的是**不依赖上游不出错**。
fn audio_endpoint_key(audio: &crate::audio::AudioDevice) -> DeviceKey {
    match audio
        .container_id
        .as_deref()
        .and_then(|c| usable_container(Some(c)))
    {
        Some(c) => DeviceKey::Container(c),
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
        let encoded = key.encode();
        grouper.add(Some(&encoded), &a.name, DevType::Audio, None, Some(a));
        // 音频端点本身已被当前音量页枚举到，说明物理设备在线；不能把
        // 「无电量字段但有端点」误当成离线占位条目。
        grouper.set_connected(&encoded, true);
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
        let group_key = d.device_key.as_deref().or(Some(fallback.as_str()));
        grouper.add(
            group_key,
            &d.name,
            d.dt,
            d.battery.map(|b| (b, source)),
            None,
        );
        if let Some(key) = group_key {
            grouper.set_wireless_24g_kind(key, d.wireless_24g_kind);
            grouper.set_connected(key, d.is_connected);
        }
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
            volume: None,
            is_muted: None,
            is_default: None,
            // 占位条目此刻枚举不到端点 ⇒ 名字/类别皆无（画默认图标），由 widget 置灰
            audio_endpoint_name: None,
            audio_kind: AudioKind::Pointer,
            // 占位条目没有参与聚合的节点 ⇒ 无类别、计数为 0、未连接
            categories: Vec::new(),
            node_count: 0,
            connected: false,
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

/// 选择器条目：设备的**来源**标记（第 3 层 T3-3「每项标注来源」用）。
///
/// 刻意不做成 `bool` 二元组 —— 一台设备可以**同时**来自两侧（这正是合并的常见结果），
/// 用位标记表达「两侧都出现」比两个布尔字段更难写错（不会出现「两个都 true 却没处理」）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceSources {
    /// 出现在设备信息页（`get_devices` 的结果里）
    pub device_page: bool,
    /// 出现在音量控制页（输出或输入端点）
    pub volume_page: bool,
}

/// 选择器条目：两页的**并集**里的一台（物理）设备。
///
/// ⛔ 与 `PhysicalDevice` 的**关键区别**：本类型**不做数据可用性过滤** ——
/// 凡出现在两页里就保留，即使读不出电量也读不出音量。理由：选择器的职责是
/// 「让用户选择要固定/显示的设备」，**不是**「显示当前数据」；用数据可用性筛掉条目
/// 会让用户根本无法为「此刻没读到数据的设备」做设置。
#[derive(Debug, Clone, Serialize)]
pub struct MergedDevice {
    /// `DeviceKey::encode()` 的结果（`c:` / `i:` / `n:` 前缀）
    pub key: String,
    /// 组内候选名按 `pick_display_name` 挑出的名字（与 `PhysicalDevice` 同一规则）
    pub name: String,
    /// 电量；两侧都没有则为 `None`（⚠️ `Some(0)` 是合法值，判空必须用 `is_none()`）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub battery: Option<i32>,
    /// 组内命中的端点 id（输出优先，详见 `merge_by_identity` 的排序约定）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_device_id: Option<String>,
    /// 出现过的设备类别（空数组表示只有音频端点参与、或键来自降级）
    pub categories: Vec<DevType>,
    /// 参与聚合的条目数（设备行 + 端点行）
    pub node_count: usize,
    /// 该设备来自哪一页（用于 T3-3 的「仅设备页 / 仅音量页」标注）
    pub sources: DeviceSources,
}

/// 校验一条**已编码**的身份键（`c:` / `i:` / `n:`）在结构上仍然合法；
/// 不合法时返回 `None`，让调用方按降级链重算。
///
/// 存在的唯一理由：**`c:` 级键必须仍然指向一个「可用容器」**。
/// `usable_container` 只在**构造**键时把关（`device_key` 内部），一旦键被编码成字符串、
/// 存进 `Device.device_key` 并跨函数/跨层传递，那个把关就不再被复核 ——
/// 若上游填入占位容器（`{…-ffffffffffff}`），占位容器上的多条无关设备就会
/// **静默并成一条**（Spec §3.1：「比现状更糟」）。
///
/// ⚠️ 今天的上游是安全的（`wmi_query.rs:242/333` 走 `device_key`，内部已过 `usable_container`），
/// 所以本函数在生产路径上**恒返回 `Some`** —— 它是**结构性防御**，不是补漏。
/// 判据刻意做得极保守：**只拒绝 `c:` 级里不可用的容器**，其余一律原样放行
/// （`i:` / `n:` 无可校验的语义，且它们的降级代价是「拆细」而非「串号」，方向安全）。
fn normalize_encoded_key(encoded: &str, name: &str) -> Option<String> {
    if let Some(raw) = encoded.strip_prefix("c:") {
        // 复用 `usable_container` —— ⛔ 绝不自己判占位（Spec §3.1 纪律）
        return usable_container(Some(raw)).map(|g| DeviceKey::Container(g).encode());
    }
    if encoded.starts_with("i:") || encoded.starts_with("n:") {
        return Some(encoded.to_string());
    }
    // 前缀未知 ⇒ 不是本模块产出的键 ⇒ 当作不可信，交给降级链
    let _ = name;
    None
}

/// 选择器数据源：`设备页 ∪ 输出端点 ∪ 输入端点`，按**物理设备身份**合并。
///
/// ⛔⛔ **本函数刻意不复用 `group_taskbar_devices`** —— 后者末尾的**反向补建循环**
/// （见该函数文档 ②）会为「被 pin 但本次枚举完全没出现」的设备**凭空造条目**，
/// 而那些设备在设备页与音量页里**根本不存在** ⇒ 会让选择器**超出并集**。
/// 见 Spec §0.4（三）与 §4 T3-1。
///
/// ⛔ 同样**不做**数据可用性过滤（那是任务栏窗口的语义，不是选择器的）。
///
/// 身份判据（**只用 `device_key`，绝不用 `name` 反推** —— 见 Spec §3.1）：
///   · 设备行：`d.device_key` 优先；为 `None` 时降级 `DeviceKey::Name(core_name(&d.name))`
///     （与 `group_taskbar_devices` 同一降级，保证两台设备不会因「一个有键一个没键」而错分）；
///   · 端点行：`audio_endpoint_key(a)` —— 有容器用容器，无容器退 `SWD\MMDEVAPI\{id}`。
///
/// ⛔ **容器串只从上述既有字段取**，不得自行拼接 / 手工解析字符串形态的 ContainerId
/// （`audio_endpoint_key` 不做归一化，责任在调用方 —— 见 Spec §6 末）。
///
/// 端点排序：同一容器可能有**多个**端点（实测一个容器覆盖过 3 个），
/// 这里先把**系统默认**端点排到前面 —— 与 `group_taskbar_devices` 的约定一致，
/// 让「哪个端点代表这台设备」有确定且符合直觉的答案。
///
/// **输入端点**（`inputs`）与输出端点同等对待：它们只在音量页的会话右键菜单可见
/// （Spec §0.4 一），但选择器要覆盖它们（验收判据 2）。
pub fn merge_by_identity(
    devices: &[crate::device::Device],
    outputs: &[crate::audio::AudioDevice],
    inputs: &[crate::audio::AudioDevice],
) -> Vec<MergedDevice> {
    let mut grouper = Grouper::new();
    let mut from_device_page: BTreeSet<String> = BTreeSet::new();
    let mut from_volume_page: BTreeSet<String> = BTreeSet::new();

    // ── 先入端点：**输出在前**，且各自内部把系统默认端点排到前面 ──────────
    // 输出先于输入 ⇒ 同一容器同时有输出与输入端点时，音量代表的是**输出**
    // （用户「调这台设备的音量」指的就是输出）。
    let mut ordered: Vec<&crate::audio::AudioDevice> =
        Vec::with_capacity(outputs.len() + inputs.len());
    let mut out_sorted: Vec<&crate::audio::AudioDevice> = outputs.iter().collect();
    out_sorted.sort_by_key(|a| !a.is_default);
    ordered.extend(out_sorted);
    let mut in_sorted: Vec<&crate::audio::AudioDevice> = inputs.iter().collect();
    in_sorted.sort_by_key(|a| !a.is_default);
    ordered.extend(in_sorted);

    for a in ordered {
        let key = audio_endpoint_key(a).encode();
        from_volume_page.insert(key.clone());
        // `audio = Some(a)` ⇒ 音量字段由端点提供；`battery = None` ⇒ 端点不提供电量
        grouper.add(Some(&key), &a.name, DevType::Audio, None, Some(a));
        grouper.set_connected(&key, true);
    }

    // ── 再入设备行：电量与类别由它提供 ────────────────────────────────────
    for d in devices {
        let source = if d.is_bluetooth || d.is_ble {
            BatterySource::Bluetooth
        } else if d.is_wireless_24g {
            BatterySource::Hid24g
        } else {
            BatterySource::Unknown
        };
        // ⛔ **键必须过 `usable_container` 重置**，不可直接信任 `d.device_key`。
        //
        // 今天的生产端是安全的：`Device.device_key` 来自
        // `wmi_query.rs:242`（PnP）与 `:333`（蓝牙）的 `device_identity::device_key(..)`，
        // 而它内部的 `usable_container` 已排除占位容器 ⇒ 占位时该字段本就是 `None`。
        //
        // ⚠️ 但那是**上游的巧合**，不是本函数的保证。若沿用 `d.device_key` 原文，
        // 一旦上游哪天把未过滤的容器串填进来（或有人给 `Device` 加一条新生产者），
        // 占位容器上的多条设备就会在这个函数里**静默并成一条** ——
        // 正是 Spec §3.1 说的「比现状更糟」。
        // ⇒ 这里对**已编码的键**做一次结构性校验：`c:` 级必须仍是可用容器，否则按降级链重算。
        //    成本是一次字符串前缀判断，换来的是「本函数自身即可保证不误并」。
        let fallback = DeviceKey::Name(core_name(&d.name)).encode();
        let key = match d.device_key.as_deref() {
            Some(k) => normalize_encoded_key(k, &d.name).unwrap_or_else(|| fallback.clone()),
            None => fallback.clone(),
        };
        from_device_page.insert(key.clone());
        grouper.add(
            Some(&key),
            &d.name,
            d.dt,
            d.battery.map(|b| (b, source)),
            None,
        );
    }

    // `Grouper::finish` 已按 key 排序（内部 `BTreeMap`）⇒ 顺序稳定，UI 不会跳动。
    // ⚠️ 这里**不做任何 filter**（尤其不做 `battery.is_some() || audio_device_id.is_some()`）。
    grouper
        .finish()
        .into_iter()
        .map(|p| MergedDevice {
            battery: p.battery,
            audio_device_id: p.audio_device_id,
            categories: p.categories,
            node_count: p.node_count,
            sources: DeviceSources {
                device_page: from_device_page.contains(&p.key),
                volume_page: from_volume_page.contains(&p.key),
            },
            key: p.key,
            name: p.name,
        })
        .collect()
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

/// 选择器里的一台设备：并集口径 + 已套用显示名的**最终**形态（第 3 层 T3-1）。
///
/// 与 `MergedDevice` 的关系：本类型是它的**装配结果** —— 多出两个「只有命令层才知道
/// 上下文」的字段（用户自定义名、是否已固定），少一个仅供 T3-3 内部使用的 `sources`
/// 之外的中间态。刻意不复用 `PhysicalDevice`：后者带 `volume` / `is_muted` /
/// `is_default`，而选择器**不需要**这些实时值（它只负责选设备，不显示音量），
/// 多带出去反而让前端误以为可以显示。
#[derive(Debug, Clone, Serialize)]
pub struct SelectableDevice {
    /// `DeviceKey::encode()` 的结果；前端原样回传给 `toggle_pinned_taskbar_device`
    pub key: String,
    /// **最终**显示名：已按 `pin.alias > resolve_device_name > 短名` 三级优先级解析
    pub name: String,
    /// 固定的**兜底键**（`n:<core_name(显示名)>`），前端原样回传给
    /// `toggle_pinned_taskbar_device` 的 `fallback` 参数。
    ///
    /// ⛔⛔ **必须由后端算好返回，绝不可让前端用 `simplifyDeviceName` 现算** ——
    /// 那个 JS 函数与 Rust 的 `core_name` **不等价**（JS 取**第一个** `(`，Rust 取 `" ("`；
    /// 且 Rust 会**剥 17 种协议后缀**而 JS 不剥）。前端一旦自算，写入的 `fallback`
    /// 就与显示侧的判据（`DeviceKey::Name(core_name(&p.name))`）**对不上** ⇒
    /// `fallback` 形同虚设，用户换机/重装驱动后固定项**静默失效**。
    /// 这与 Spec §9「复用同一判据必须参数逐字一致」是同一条纪律。
    pub fallback: String,
    /// 该设备来自哪一页（T3-3 的「仅设备页 / 仅音量页」标注）
    ///
    /// ⚠️ **当前前端不使用**（用户 2026-09-24 决定「不用标注来源」，T3-3 已取消）。
    /// 保留字段的理由：它是并集的**固有信息**，后端算好只花一次 `BTreeSet` 查找；
    /// 删掉则将来想加回标注必须重跑一遍合并逻辑。若确定永不需要，可连同
    /// `DeviceSources` 一起删除（届时 `merge_by_identity` 内的两个 `BTreeSet` 也可去掉）。
    pub sources: DeviceSources,
    /// 是否已在任务栏信息窗里固定（复选框的**初始勾选态**）
    pub pinned: bool,
}

/// 把并集结果装配成选择器条目：解析最终显示名 + 标注固定态。
///
/// ── 显示名三级优先级（Spec §7 第 8 条）────────────────────────────────────
///   1. `pin.alias`          —— 用户在「固定」时设的别名，优先级最高（它就是为这个设备设的）；
///   2. `resolve_device_name` —— 全局自定义名（覆盖两页的键口径差异）；
///   3. 并集自带的 `name`     —— `pick_display_name` 挑出的短名兜底。
///
/// ⛔ **本函数不做 pin 补建**（决策 7）：只对**并集里真实存在**的设备判定固定态。
/// `group_taskbar_devices` 那条「为 pinned 但本次枚举没出现的设备凭空造条目」的循环
/// **绝不能**引入 —— 那会让选择器出现「两页都没有、用户也没法取消」的幽灵条目。
///
/// 判据复用 `config::matches_pinned_taskbar`（与显示侧同一份），避免「勾选态」与
/// 「任务栏实际显示」两套规则分叉。
pub fn build_selectable_devices(
    merged: &[MergedDevice],
    pinned: &[crate::config::PinnedDevice],
    config: &crate::config::Config,
) -> Vec<SelectableDevice> {
    merged
        .iter()
        .map(|m| {
            // 固定态与别名一起取：命中哪条 pin，就用它的 alias —— 两者必须来自**同一条**记录，
            // 否则会出现「已固定但用着别的设备的别名」。
            //
            // ⛔ **`fallback` 必须与显示侧（本文件 `group_taskbar_devices` 的
            // `p.pinned = …` 那一行）逐字一致**：那边传的是 `n:core_name(显示名)`。
            // 若这里偷懒传 `None`，「`key` 存容器 + `fallback` 存名称键」这类 pin
            // （`PinnedDevice` 文档推荐的形态，用于容忍换机/重装驱动）就会在这里失配：
            // 显示侧说「已固定」、选择器却显示未勾选 ⇒ 用户点一下实际走的是**新增**分支，
            // 任务栏条目反而消失。两处口径分叉正是这条判据要防的事。
            let fallback = DeviceKey::Name(core_name(&m.name)).encode();
            let hit = pinned
                .iter()
                .find(|p| crate::config::pinned_device_matches(p, &m.key, Some(&fallback)));
            let name = hit
                .and_then(|p| p.alias.as_deref())
                .map(str::trim)
                .filter(|a| !a.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| crate::config::resolve_device_name(&m.name, config));
            SelectableDevice {
                key: m.key.clone(),
                name,
                fallback,
                sources: m.sources,
                pinned: hit.is_some(),
            }
        })
        .collect()
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

    /// ⭐⭐ **T1-0.5（前置门）：PnP 侧 `device_key` 与音频侧 `audio_endpoint_key`
    /// 对**同一条音频端点**必须产出逐字相同的键。**
    ///
    /// 为什么这条单测必须先于 T1-1 存在：
    /// 任务栏设备选择器的第 1 层（`merge_by_identity`）**整条合并判据**建立在这个等式上。
    /// 此前只有 `probe-keyalign.py`（用 **Python 重实现**两个 key 函数）得出「相等」的结论
    /// —— 那是**文档级验证，不是可执行验证**。若两键实际不逐字相等，
    /// 合并会在真机上**静默不合并**（不报错、不告警、不 panic），
    /// 而用手写 key 的单测**照样全绿**。**这是最危险的一类假验收。**
    ///
    /// 端点串取自真机日志（`debug_once_8932.log`）里的 PnPEntity `PNPDeviceID` 列：
    /// `classify_device: 扬声器 (网易虚拟音频设备) -> Audio (pnp_class=AudioEndpoint, pnp_id=…)`
    ///
    /// 可证伪：把 `audio_endpoint_key` 的 `None` 分支改成
    /// `DeviceKey::Name(...)`（或改动 `format!("SWD\\MMDEVAPI\\{}")` 的拼接形式）即转红。
    #[test]
    fn pnp_device_key_matches_audio_endpoint_key_for_same_endpoint() {
        // 真机日志里的逐字串（大写，含花括号）—— PnP 侧 `PNPDeviceID` 的原样形态
        const PNP_ID: &str =
            "SWD\\MMDEVAPI\\{0.0.0.00000000}.{C5C40F0A-A935-48A4-BC19-06FF1D09E504}";
        // 音频侧 `AudioDevice.id` 的原样形态（无 `SWD\MMDEVAPI\` 前缀）
        const AUDIO_ID: &str = "{0.0.0.00000000}.{C5C40F0A-A935-48A4-BC19-06FF1D09E504}";

        // ① 设备侧：PnP 行的实例路径就是上面那条串（无容器 ⇒ 降级到 Instance）
        let from_pnp = device_key(None, Some(PNP_ID), None).unwrap();

        // ② 音频侧：无真实容器（虚拟设备落占位容器 ⇒ container_id 为 None）⇒ 同样降级
        let from_audio = audio_endpoint_key(&audio("扬声器 (网易虚拟音频设备)", AUDIO_ID, None));

        assert_eq!(
            from_pnp, from_audio,
            "同一音频端点在 PnP 侧与音频侧必须得到同一个身份键，\
             否则第 1 层合并在真机上会静默失效"
        );

        // ③ 钉住形态：必须是 `i:` 级、且已全小写（`DeviceKey::encode` 对 Instance 做 lowercase）
        assert_eq!(
            from_audio.encode(),
            "i:swd\\mmdevapi\\{0.0.0.00000000}.{c5c40f0a-a935-48a4-bc19-06ff1d09e504}",
            "实例路径必须全小写并带 i: 前缀"
        );

        // ④ 反控：容器存在时**两侧都**走 Container 级 —— 音频侧不得只看 id 而忽略容器。
        //    （避免有人「简化」成永远用 Instance，那样真机上有容器的设备就合不上了）
        //
        //    ⚠️ 这里刻意喂**归一化形态**的容器串 —— 因为 `AudioDevice.container_id` 的唯一
        //    生产者是 `container_of_instance` → `usable_container(format_guid_bytes(..))`，
        //    它**必然**产出小写无花括号形态。见下面 ④b 的契约钉板。
        const C: &str = "40e11c06-72bd-5b38-9bd2-0e15079b3b45";
        let pnp_c = device_key(Some(C), Some(PNP_ID), None).unwrap();
        let audio_c = audio_endpoint_key(&audio("扬声器 (DUNU DTC100pro)", AUDIO_ID, Some(C)));
        assert_eq!(pnp_c, audio_c, "有真实容器时两侧都必须走容器级");
        assert_eq!(pnp_c.encode(), "c:40e11c06-72bd-5b38-9bd2-0e15079b3b45");

        // ④b ⛔ **契约钉板（本单测的真正价值所在）**：
        //    两条路径对容器 GUID 的归一化责任**曾经不同** ——
        //      · `device_key(Some(raw), ..)` 内部经 `usable_container` ⇒ **会** `normalize_guid`；
        //      · `audio_endpoint_key(..)` 曾是 `DeviceKey::Container(c.clone())` ⇒ **裸克隆，不归一化**。
        //    ⇒ 那时它把「容器串必须已归一化」的责任**推给了调用方**；新调用方（`merge_by_identity`）
        //      若从别处取得容器串，就会**静默分叉** —— 键不等 ⇒ 不合并、不报错。
        //
        //    ⭐ **本钉板首次运行时抓到了更糟的一种：占位容器被原样接受**
        //      （探针实测：两条落在占位容器上的端点被并成 1 条）。
        //    ⇒ **已修**：`audio_endpoint_key` 现在自己过 `usable_container`（责任收回自己承担）。
        //      这条断言随之更新为钉住**修复后**的契约：
        //      未归一化输入应被**归一化**（而非原样保留），占位容器应被**降级**（而非当成容器）。
        assert_eq!(
            audio_endpoint_key(&audio(
                "x",
                AUDIO_ID,
                Some("{40E11C06-72BD-5B38-9BD2-0E15079B3B45}")
            ))
            .encode(),
            "c:40e11c06-72bd-5b38-9bd2-0e15079b3b45",
            "契约：audio_endpoint_key 必须自己归一化容器串（不得依赖调用方）"
        );
        assert_eq!(
            device_key(
                Some("{40E11C06-72BD-5B38-9BD2-0E15079B3B45}"),
                Some(PNP_ID),
                None
            )
            .unwrap()
            .encode(),
            "c:40e11c06-72bd-5b38-9bd2-0e15079b3b45",
            "对照：device_key 侧同样归一化 ⇒ 两侧责任现在**对称**"
        );

        // ⑤ 反控：**占位容器**不得把两侧粘在一起 —— 占位容器在设备侧被
        //    `usable_container` 拒掉、在音频侧 `container_id` 恒为 None（见 audio.rs 文档），
        //    两侧都降级到 Instance，仍然是同一条键（这正是虚拟设备能对齐的原因）。
        let audio_placeholder = audio_endpoint_key(&audio(
            "扬声器 (Steam Streaming Speakers)",
            AUDIO_ID,
            None, // container_of_audio_endpoint 已过 usable_container ⇒ 占位容器在这里就是 None
        ));
        assert_eq!(
            audio_placeholder, from_pnp,
            "占位容器在音频侧表现为 None ⇒ 仍能与 PnP 侧对齐"
        );
        assert!(
            !audio_placeholder.encode().starts_with("c:"),
            "占位容器绝不可被当成真实容器"
        );
    }

    /// T1-0.5 的**反向自检**：等式不是恒真 —— 换成**另一条**端点必须得到**不同的**键。
    ///
    /// 上面那条单测若因为两个函数都「退化成常量」而通过，本用例会把它暴露出来。
    /// 可证伪：把 `audio_endpoint_key` 改成返回固定值即转红。
    #[test]
    fn different_audio_endpoints_produce_different_keys() {
        const A: &str = "{0.0.0.00000000}.{C5C40F0A-A935-48A4-BC19-06FF1D09E504}";
        const B: &str = "{0.0.0.00000000}.{91AC81AF-0000-0000-0000-000000000000}";

        let ka = audio_endpoint_key(&audio("扬声器 (网易虚拟音频设备)", A, None)).encode();
        let kb = audio_endpoint_key(&audio("扬声器 (Mijia)", B, None)).encode();

        assert_ne!(ka, kb, "不同端点必须得到不同键，否则等式是恒真的（假绿）");
        assert!(ka.contains(&A.to_ascii_lowercase()));
        assert!(kb.contains(&B.to_ascii_lowercase()));
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
            wireless_24g_kind: None,
            is_connected: true,
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
        // 音量侧的标记必须与**被选中的那条端点**同源：选中的是默认端点，`is_default`
        // 就该为真、`volume` 就该是它的 0.5 —— 不得出现「id 取自 A、标记取自 B」的错配。
        assert_eq!(
            out[0].is_default,
            Some(true),
            "四个音频字段必须由同一条端点派生"
        );
        assert_eq!(out[0].volume, Some(0.5), "音量取自被选中的那个端点");
    }

    /// 音量侧的信息必须**随设备一起返回**，不能让前端自己去 `get_audio_devices` 里
    /// 按 id join —— 那样「`audio_device_id` 匹配 `AudioDevice.id`」就成了未文档化的
    /// **隐式契约**，前端写错（例如改用 `name` 匹配）既不报错也不告警，只会静默显示
    /// 错误的音量。聚合时 `AudioDevice` 本来就在手边，带上它零成本。
    ///
    /// 判据可证伪：把 `finish()` 里的 `volume: audio.as_ref().map(|a| a.volume)`
    /// 改成 `None`，本用例必转红。
    #[test]
    fn audio_volume_details_ride_along_with_the_device() {
        let mut endpoint = audio(
            "耳机 (小爱音箱-9205)",
            "{0.0.0.0}.{abc}",
            Some("9039aea7-9c07-52f2-a4ef-0f5296d6d7d2"),
        );
        endpoint.volume = 0.42;
        endpoint.is_muted = true;

        let out = group_taskbar_devices(&[], &[endpoint], &[]);
        assert_eq!(out.len(), 1, "只有一条音频端点：{out:?}");
        let d = &out[0];
        assert_eq!(d.audio_device_id.as_deref(), Some("{0.0.0.0}.{abc}"));
        assert!(
            matches!(d.volume, Some(v) if (v - 0.42).abs() < 1e-6),
            "音量必须随设备返回，实际 {:?}",
            d.volume
        );
        assert_eq!(d.is_muted, Some(true), "静音状态必须随设备返回");
        assert_eq!(d.is_default, Some(false), "非默认端点必须如实标记");
    }

    /// 反控：**没有音频端点**的设备（键鼠）三个音量字段必须都是 `None`
    /// —— 否则前端会把「无音量」误当成「音量为 0 / 未静音」。
    ///
    /// 判据可证伪：把 `finish()` 里的 `volume` 改成 `Some(0.0)`，本用例必转红。
    #[test]
    fn device_without_audio_endpoint_has_no_volume_fields() {
        let devices = vec![dev("罗技接收器", Some("c:kb-only"), Some(90), false, true)];
        let out = group_taskbar_devices(&devices, &[], &[]);
        assert_eq!(out.len(), 1);
        let d = &out[0];
        assert_eq!(d.battery, Some(90), "电量侧照常");
        assert!(d.audio_device_id.is_none());
        assert!(d.volume.is_none(), "无音频端点 ⇒ 音量必须是 None 而非 0");
        assert!(d.is_muted.is_none());
        assert!(d.is_default.is_none());
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

    // ── 第 1 层：选择器并集合并（T1-1）─────────────────────────────

    /// 验收判据 3：**同一台设备在两侧名字不同时仍合并为一条**
    /// —— `扬声器 (DUNU DTC100pro)`（音量页）与 `DUNU DTC100pro`（设备页）。
    ///
    /// 这是 DUNU 的核心回归：设备页行有 `device_key`（容器）、
    /// 音量页端点走 `audio_endpoint_key`（同容器）⇒ 两者**必须**并成一条。
    #[test]
    fn merge_by_identity_joins_two_sides_with_different_names() {
        const CID: &str = "40e11c06-72bd-5b38-9bd2-0e15079b3b45";
        let key = format!("c:{CID}");
        let devices = vec![dev("DUNU DTC100pro", Some(&key), Some(100), false, false)];
        let outputs = vec![audio(
            "扬声器 (DUNU DTC100pro)",
            "{0.0.0.0}.{dunu}",
            Some(CID),
        )];

        let out = merge_by_identity(&devices, &outputs, &[]);

        assert_eq!(out.len(), 1, "两侧必须并成一条，实际 {out:?}");
        assert_eq!(out[0].key, key);
        assert_eq!(out[0].battery, Some(100), "电量来自设备行");
        assert_eq!(out[0].audio_device_id.as_deref(), Some("{0.0.0.0}.{dunu}"));
        assert_eq!(out[0].node_count, 2);
        // 展示名取「括号内设备名」与设备页短名的最长者 —— 两者 core_name 相同 ⇒ 结果唯一
        assert_eq!(out[0].name, "DUNU DTC100pro");
        assert!(
            out[0].sources.device_page && out[0].sources.volume_page,
            "两侧都标到"
        );
    }

    /// 验收判据 4：**占位容器上的多条设备不得互相合并**（网易×2 + Steam×1 必须仍是 3 条）。
    ///
    /// ⛔ 这是「比现状更糟」的防线：三条虚拟设备挤在
    /// `{00000000-0000-0000-FFFF-FFFFFFFFFFFF}`，若 `merge_by_identity` 自己判容器
    /// （而不复用 `usable_container`）就会并成一条。
    ///
    /// ⚠️ 用例必须用**无容器**的真实形态喂入：`container_of_audio_endpoint` 已过
    /// `usable_container` ⇒ 占位容器在 `AudioDevice.container_id` 上**表现为 `None`**，
    /// 于是三条各走 `i:SWD\MMDEVAPI\{id}`、id 互不相同 ⇒ 天然不合并。
    /// ⛔ 若改成「手动喂占位容器串」，测的就是**另一条代码路径**（`device_key` 的排除逻辑），
    /// 见下一条单测。两条必须都在，否则会漏掉一半。
    #[test]
    fn merge_by_identity_keeps_placeholder_container_devices_separate() {
        let outputs = vec![
            audio("扬声器 (网易虚拟音频设备)", "{0.0.0.0}.{netease-out}", None),
            audio(
                "扬声器 (Steam Streaming Speakers)",
                "{0.0.0.0}.{steam}",
                None,
            ),
        ];
        let inputs = vec![audio(
            "麦克风阵列 (网易虚拟音频设备)",
            "{0.0.1.0}.{netease-in}",
            None,
        )];

        let out = merge_by_identity(&[], &outputs, &inputs);

        assert_eq!(out.len(), 3, "三条虚拟设备必须各自独立，实际 {out:?}");
        assert!(
            out.iter().all(|d| d.key.starts_with("i:")),
            "无容器 ⇒ 走 i: 级"
        );
        // 同一台「网易」的输出与输入端点 id 不同 ⇒ 仍是两条（这是已知且可接受的语义：
        // 端点级身份，不是物理设备级 —— 本机这两条确实落在同一占位容器上）
        let names: Vec<&str> = out.iter().map(|d| d.name.as_str()).collect();
        assert!(
            names.contains(&"Steam Streaming Speakers"),
            "实际 {names:?}"
        );
    }

    /// ⛔ **防御性契约**：即使有人把**占位容器**填进 `Device.device_key`
    /// （今天 `wmi_query.rs:242/333` 不会 —— 它走 `device_key`，内部已过 `usable_container`），
    /// `merge_by_identity` 也必须**自己复核**，不得把「键已过滤」的责任全押给上游。
    ///
    /// ⚠️ 本用例**首次运行时确实转红了** —— 暴露了初版实现直接信任 `d.device_key`
    /// （三条占位容器设备被并成一条）。修复方式是在本函数内加 `normalize_encoded_key`
    /// 复核 `c:` 级键，而非删掉用例。
    ///
    /// 理由：这是 T1-0.5 记下的**同一类**问题 —— 一个函数把不变量的把关责任推给调用方，
    /// 今天安全、明天脆弱，且失配时**静默**（不报错）。上游一动，这里就悄悄坏。
    /// ⛔ **防御性契约（音频侧）**：若 `AudioDevice.container_id` 被填入**占位容器**
    /// （今天 `container_of_audio_endpoint` 已过 `usable_container` ⇒ 传进来就是 `None`），
    /// `audio_endpoint_key` 也必须**自己降级**，不得原样接受。
    ///
    /// ⚠️ 本用例来自一次**探针实测**：修复前，两条落在占位容器上的端点被并成 **1 条**
    /// （键 `c:{00000000-0000-0000-FFFF-FFFFFFFFFFFF}`）——
    /// 与设备侧是**同一个洞**，只是当时只堵了设备侧。**这就是「防御必须对称」的证据。**
    #[test]
    fn merge_by_identity_never_trusts_upstream_null_container_on_audio_side() {
        const NULL_C: &str = "{00000000-0000-0000-FFFF-FFFFFFFFFFFF}";
        let outputs = vec![
            audio("扬声器 (网易虚拟音频设备)", "{0.0.0.0}.{a}", Some(NULL_C)),
            audio(
                "扬声器 (Steam Streaming Speakers)",
                "{0.0.0.0}.{b}",
                Some(NULL_C),
            ),
        ];

        let out = merge_by_identity(&[], &outputs, &[]);

        assert_eq!(
            out.len(),
            2,
            "占位容器上的两条端点绝不可并成一条（修复前实测为 1 条），实际 {out:?}"
        );
        assert!(
            out.iter().all(|d| d.key.starts_with("i:")),
            "占位容器必须降级到 i: 级，实际 {:?}",
            out.iter().map(|d| &d.key).collect::<Vec<_>>()
        );
        assert!(
            !out.iter().any(|d| d.key.contains("ffffffffffff")),
            "占位容器绝不可出现在输出键里"
        );
        // 单函数级：直接钉住 `audio_endpoint_key` 的行为
        assert_eq!(
            audio_endpoint_key(&outputs[0]).encode(),
            format!("i:swd\\mmdevapi\\{}", outputs[0].id.to_ascii_lowercase()),
            "占位容器 ⇒ 降级到端点自己的实例路径"
        );
    }

    #[test]
    fn merge_by_identity_never_trusts_upstream_null_container_key() {
        // ⚠️ 必须是**已编码**形态（带 `c:` 前缀）—— 这才是 `Device.device_key` 的真实形态
        //    （`wmi_query.rs` 里 `.map(|k| k.encode())`）。
        //    若传裸 GUID，`normalize_encoded_key` 会走「前缀未知」分支而**与拆掉防御同路**，
        //    用例便失去区分力（这一点在 T1-2 注入时被实测抓到，故在此显式标注）。
        const NULL_C: &str = "c:00000000-0000-0000-ffff-ffffffffffff";
        // 三条不同设备**错误地**共享同一个占位容器（真机实测形态）
        let devices = vec![
            dev(
                "扬声器 (网易虚拟音频设备)",
                Some(NULL_C),
                None,
                false,
                false,
            ),
            dev(
                "扬声器 (Steam Streaming Speakers)",
                Some(NULL_C),
                None,
                false,
                false,
            ),
            dev(
                "麦克风阵列 (网易虚拟音频设备)",
                Some(NULL_C),
                None,
                false,
                false,
            ),
        ];

        let out = merge_by_identity(&devices, &[], &[]);

        // ⚠️ 降级到 `n:core_name(name)` 后，**两条「网易」会并成一条** ——
        // 因为它们的 `core_name` 都是 `网易虚拟音频设备`。这是 `n:` 级的**固有代价**
        // （按名字合并），方向是「可能少一条」，**不是**「把无关设备串成一台」。
        // ⇒ 只有 2 条：`网易虚拟音频设备`（合并）+ `Steam Streaming Speakers`。
        // ⛔ **关键**：若不复核而直接信任 `c:` 键，则会并成 **1 条**（全落在同一占位容器键下）
        //    —— 这个 2 vs 1 的差值就是本用例的区分力所在（T1-2 注入已验证）。
        assert_eq!(
            out.len(),
            2,
            "占位容器必须被拒；降级后按 core_name 合并 ⇒ 2 条（不拒则为 1 条），实际 {out:?}"
        );
        assert!(
            out.iter().all(|d| d.key.starts_with("n:")),
            "占位容器被 `usable_container` 拒掉 ⇒ 降级到 n: 级；实际 {:?}",
            out.iter().map(|d| &d.key).collect::<Vec<_>>()
        );
        // ⭐ 关键断言：**绝不存在 `c:` 级的占位容器键**
        assert!(
            !out.iter().any(|d| d.key.contains("ffffffffffff")
                || d.key.contains("00000000-0000-0000-0000-000000000000")),
            "占位容器绝不可出现在输出键里，实际 {:?}",
            out.iter().map(|d| &d.key).collect::<Vec<_>>()
        );
    }

    /// 降级链一致性：**设备行无键**（如来自 `Win32_Battery` 的 `device_key: None`）
    /// 与**端点无容器**不得被**混为两台**，也不得被**错并成一台**。
    ///
    /// · 无键设备行 ⇒ 降级 `n:core_name(name)`，两台不同名的设备仍是两条；
    /// · 端点无容器 ⇒ `i:SWD\MMDEVAPI\{id}`；
    /// ⇒ 两者**不可能**相等（前缀都不同）⇒ 各自独立。这是**保守方向**（拆细而非串号）。
    #[test]
    fn merge_by_identity_keyless_device_and_containerless_endpoint_stay_separate() {
        let devices = vec![
            dev("DUNU DTC100pro", None, Some(90), false, false),
            dev("Mijia", None, Some(70), false, false),
        ];
        let outputs = vec![audio("扬声器 (DUNU DTC100pro)", "{0.0.0.0}.{dunu}", None)];

        let out = merge_by_identity(&devices, &outputs, &[]);

        // 无键设备行降级到 n: 级、端点走 i: 级 ⇒ 三条互不相同
        assert_eq!(out.len(), 3, "不同前缀的键不可合并，实际 {out:?}");
        assert_eq!(out.iter().filter(|d| d.key.starts_with("n:")).count(), 2);
        assert_eq!(out.iter().filter(|d| d.key.starts_with("i:")).count(), 1);
    }

    /// ⛔ **不做数据可用性过滤** —— 与 `group_taskbar_devices` 的关键语义差异。
    ///
    /// 一台设备若「无电量、无音量」，任务栏窗口会丢弃它（对用户无意义），
    /// 但**选择器必须保留** —— 否则用户无法为它做任何设置（这正是 Spec §0.4 的纪律）。
    #[test]
    fn merge_by_identity_keeps_devices_without_any_data() {
        let devices = vec![dev(
            "某无从读数的鼠标",
            Some("c:aaaa-bbbb"),
            None,
            false,
            false,
        )];

        let out = merge_by_identity(&devices, &[], &[]);

        assert_eq!(out.len(), 1, "选择器不做数据可用性过滤，实际 {out:?}");
        assert_eq!(out[0].battery, None);
        assert_eq!(out[0].audio_device_id, None);
        assert_eq!(out[0].node_count, 1);
        assert!(out[0].sources.device_page && !out[0].sources.volume_page);
    }

    /// 幂等 / 确定性：同样输入重复调用、以及**输入顺序打乱**，结果必须逐字相同。
    ///
    /// `Grouper` 内部是 `BTreeMap` ⇒ 输出按 key 排序，顺序稳定（UI 不会跳）。
    /// 这是 `Grouper` 的既有契约，本函数必须继承 —— 用断言把它钉住。
    #[test]
    fn merge_by_identity_is_order_independent_and_deterministic() {
        // ⚠️ `audio()` 的 container 参数吃**裸 GUID**（与 `AudioDevice.container_id` 一致），
        // 而 `dev()` 的 key 参数吃**已编码键**（与 `Device.device_key` 一致）——
        // 两者形态不同，正是生产里的真实形态。混用会得到 `c:c:...` 双前缀的假键。
        const CID: &str = "11111111-2222-3333-4444-555555555555";
        const KEY_B: &str = "c:11111111-2222-3333-4444-555555555555";
        let d1 = dev("B 设备", Some(KEY_B), Some(50), false, false);
        let d2 = dev(
            "A 设备",
            Some("c:00000000-0000-0000-0000-0000000000aa"),
            None,
            false,
            false,
        );
        let a1 = audio("扬声器 (B 设备)", "{0.0.0.0}.{b}", Some(CID));

        // 真正测「一次调用内输入顺序相反 ⇒ 输出相同」：把同一批输入反转后再跑一次
        let devs = vec![d1.clone(), d2.clone()];
        let auds = vec![a1.clone()];
        let fwd = merge_by_identity(&devs, &auds, &[]);
        let mut devs_rev = devs.clone();
        devs_rev.reverse();
        let mut auds_rev = auds.clone();
        auds_rev.reverse();
        let rev = merge_by_identity(&devs_rev, &auds_rev, &[]);

        let keys = |v: &[MergedDevice]| v.iter().map(|d| d.key.clone()).collect::<Vec<_>>();
        assert_eq!(keys(&fwd), keys(&rev), "顺序必须与输入顺序无关");
        assert!(
            keys(&fwd).windows(2).all(|w| w[0] <= w[1]),
            "输出必须按 key 有序，实际 {:?}",
            keys(&fwd)
        );
        // 幂等：同样的输入再跑一次，结果逐字相同
        let again = merge_by_identity(
            &[
                dev("B 设备", Some(KEY_B), Some(50), false, false),
                dev(
                    "A 设备",
                    Some("c:00000000-0000-0000-0000-0000000000aa"),
                    None,
                    false,
                    false,
                ),
            ],
            &[audio("扬声器 (B 设备)", "{0.0.0.0}.{b}", Some(CID))],
            &[],
        );
        assert_eq!(keys(&fwd), keys(&again), "重复调用结果必须一致");
        // 「B 设备」（设备行 + 端点，同容器 ⇒ 1 条）+「A 设备」（1 条）= 2 条
        assert_eq!(fwd.len(), 2, "两条不同容器 ⇒ 两条，实际 {fwd:?}");
        assert_eq!(keys(&fwd)[1], KEY_B, "并起来的那条键应是设备侧原始键");
    }

    /// 输出优先于输入：同一容器同时有输出与输入端点时，`audio_device_id`
    /// 必须指向**输出**端点 —— 「调这台设备的音量」指的必然是输出。
    #[test]
    fn merge_by_identity_prefers_output_endpoint_over_input() {
        const CID: &str = "084fb1b9-0000-0000-0000-000000000001";
        let outputs = vec![audio("扬声器 (某声卡)", "{0.0.0.0}.{out}", Some(CID))];
        let inputs = vec![audio("麦克风 (某声卡)", "{0.0.1.0}.{in}", Some(CID))];

        let out = merge_by_identity(&[], &outputs, &inputs);

        assert_eq!(out.len(), 1, "同容器并成一条");
        assert_eq!(
            out[0].audio_device_id.as_deref(),
            Some("{0.0.0.0}.{out}"),
            "音量代表端点必须是输出"
        );
        assert!(out[0].sources.volume_page);
    }

    /// 同容器**多个输出端点**时取**系统默认**那个（与 `group_taskbar_devices` 同一约定）。
    #[test]
    fn merge_by_identity_picks_default_output_when_container_has_many() {
        const CID: &str = "084fb1b9-0000-0000-0000-000000000002";
        let mut non_default = audio("扬声器 A", "{0.0.0.0}.{a}", Some(CID));
        non_default.is_default = false;
        let mut default = audio("扬声器 B", "{0.0.0.0}.{b}", Some(CID));
        default.is_default = true;

        // 刻意把非默认端点放在前面 —— 排序逻辑必须把它压到后面
        let out = merge_by_identity(&[], &[non_default, default], &[]);

        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].audio_device_id.as_deref(),
            Some("{0.0.0.0}.{b}"),
            "应取系统默认端点"
        );
    }

    /// ⛔⛔ **验收判据 8 的单元级版本**：`merge_by_identity` **绝不凭空造条目**。
    ///
    /// 这是它与 `group_taskbar_devices` 的**根本区别** ——
    /// 后者会为「被 pin 但未枚举到」的设备补建条目，而那些设备两页里都不存在。
    /// 本函数**没有 `pinned` 参数**，构造上不可能补建；
    /// 本单测用一个「空输入」把它钉死（防止将来有人「顺手」把 pin 逻辑接进来）。
    #[test]
    fn merge_by_identity_never_synthesizes_entries_from_nothing() {
        let out = merge_by_identity(&[], &[], &[]);
        assert!(
            out.is_empty(),
            "空输入必须空输出（不得凭空补建），实际 {out:?}"
        );

        // 对照：`group_taskbar_devices` 在同样的空输入下、有 pin 配置时**会**补建。
        // 这个对照本身就是「禁止复用」理由的可执行证据。
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:deadbeef-0000-0000-0000-000000000000".to_string(),
            fallback: None,
            alias: Some("未连接的耳机".to_string()),
        }];
        let window = group_taskbar_devices(&[], &[], &pinned);
        assert_eq!(window.len(), 1, "任务栏窗口会补建（这是它的语义）");
        assert_eq!(window[0].name, "未连接的耳机");
        assert_eq!(window[0].node_count, 0, "补建条目的标志");
        // ⇒ 若选择器复用 `group_taskbar_devices`，这条假 pin 就会**超出并集**地出现
        assert!(
            merge_by_identity(&[], &[], &[]).is_empty(),
            "选择器必须不含它"
        );
    }

    // ── T3-1：`build_selectable_devices` 装配 ──────────────────────────────

    /// ⛔⛔ **验收判据 8 的装配级版本**：选择器**不出现**「两页都没有的假 pin 条目」。
    ///
    /// 与单元级那条的区别：这里喂**非空**并集（两页都有东西），同时给一条指向
    /// **不存在设备**的 pin。若装配时误把 `group_taskbar_devices` 的补建循环搬进来，
    /// 就会多出一条「未连接的耳机」；本判据直接把它钉死。
    #[test]
    fn selectable_devices_never_include_pin_only_entries() {
        const CID: &str = "084fb1b9-1111-2222-3333-444444444444";
        let key = format!("c:{CID}");
        let devices = vec![dev("小爱音箱-9205", Some(&key), Some(80), true, false)];
        let outputs = vec![audio("扬声器 (小爱音箱-9205)", "out-a", Some(CID))];
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:deadbeef-0000-0000-0000-000000000000".to_string(),
            fallback: None,
            alias: Some("未连接的耳机".to_string()),
        }];

        let merged = merge_by_identity(&devices, &outputs, &[]);
        let sel = build_selectable_devices(&merged, &pinned, &crate::config::Config::default());

        assert_eq!(sel.len(), 1, "只应有并集里的那一台，实际 {sel:?}");
        assert!(
            sel.iter().all(|s| s.name != "未连接的耳机"),
            "伪造的 pin 别名绝不得出现"
        );
        // 对照：同样输入喂给显示侧，它会补建 ⇒ 证明这条判据确实有区分力
        let window = group_taskbar_devices(&devices, &outputs, &pinned);
        assert_eq!(window.len(), 2, "显示侧会补建第二行");
        assert!(
            window.iter().any(|d| d.name == "未连接的耳机"),
            "显示侧补建的正是选择器必须排除的那条"
        );
    }

    /// 显示名三级优先级：`pin.alias` > `resolve_device_name`（自定义名）> 短名。
    #[test]
    fn selectable_devices_prioritize_alias_over_custom_name_over_short_name() {
        const CID_A: &str = "084fb1b9-0000-0000-0000-00000000000a";
        const CID_B: &str = "084fb1b9-0000-0000-0000-00000000000b";
        const CID_C: &str = "084fb1b9-0000-0000-0000-00000000000c";
        let key_a = format!("c:{CID_A}");
        let key_b = format!("c:{CID_B}");
        let key_c = format!("c:{CID_C}");
        let devices = vec![
            dev("耳机A (设备甲)", Some(&key_a), None, false, false),
            dev("耳机B (设备乙)", Some(&key_b), None, false, false),
            dev("耳机C (设备丙)", Some(&key_c), None, false, false),
        ];
        let mut cfg = crate::config::Config::default();
        // 甲、乙：只有全局自定义名
        cfg.device_names
            .insert("设备甲".to_string(), "自定义甲".to_string());
        cfg.device_names
            .insert("设备乙".to_string(), "自定义乙".to_string());
        // 丙：没有任何自定义名 ⇒ 落到短名兜底

        let pinned = vec![crate::config::PinnedDevice {
            key: key_a.clone(),
            fallback: None,
            alias: Some("别名甲".to_string()),
        }];

        let merged = merge_by_identity(&devices, &[], &[]);
        let sel = build_selectable_devices(&merged, &pinned, &cfg);
        let by = |k: &str| sel.iter().find(|s| s.key == k).map(|s| s.name.as_str());

        assert_eq!(by(&key_a), Some("别名甲"), "alias 优先于自定义名");
        assert_eq!(by(&key_b), Some("自定义乙"), "无 alias 时用自定义名");
        assert_eq!(by(&key_c), Some("设备丙"), "都无则用短名兜底");
        // ⚠️ 丙若被错当成自定义名会显示「耳机C」—— 那说明 `pick_display_name` 没生效
        assert_ne!(by(&key_c), Some("耳机C"));
    }

    /// 固定态判据：`fallback` 必须与显示侧一致（`n:core_name(显示名)`）。
    ///
    /// 场景就是 `PinnedDevice` 文档推荐的形态 —— `key` 存容器、`fallback` 存名称键；
    /// 容器变化后只有 fallback 能认出来。若装配时给 `pinned_device_matches` 传 `None`，
    /// 这里就会判成「未固定」，而显示侧仍显示它 ⇒ 用户点一下反而把条目弄没了。
    #[test]
    fn selectable_devices_match_pinned_with_fallback_like_its_display_side() {
        // 本次枚举到的是**新容器**（换机 / 重装驱动后的常见结果）
        const CID_NEW: &str = "084fb1b9-0000-0000-0000-0000000000ff";
        let key_new = format!("c:{CID_NEW}");
        let devices = vec![dev(
            "耳机 (DUNU DTC100pro)",
            Some(&key_new),
            None,
            false,
            false,
        )];
        // pin 里存的还是**旧容器** + 名称兜底
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:00000000-0000-0000-0000-0000000000aa".to_string(),
            fallback: Some(DeviceKey::Name(core_name("DUNU DTC100pro")).encode()),
            alias: None,
        }];

        let merged = merge_by_identity(&devices, &[], &[]);
        let sel = build_selectable_devices(&merged, &pinned, &crate::config::Config::default());

        assert_eq!(sel.len(), 1);
        assert!(
            sel[0].pinned,
            "容器变了但名称兜底命中 ⇒ 必须判为已固定（否则与显示侧分叉）"
        );
        // 对照：显示侧用同一份判据，结论必须一致
        let window = group_taskbar_devices(&devices, &[], &pinned);
        assert_eq!(
            window[0].pinned, sel[0].pinned,
            "选择器与显示侧的固定态必须同真同假"
        );
    }

    /// 来源标注：仅设备页 / 仅音量页 / 两侧都有，三种都要标对（T3-3 的数据来源）。
    #[test]
    fn selectable_devices_report_sources_for_each_case() {
        const CID_BOTH: &str = "084fb1b9-0000-0000-0000-0000000000b1";
        const CID_VOL: &str = "084fb1b9-0000-0000-0000-0000000000b2";
        let key_both = format!("c:{CID_BOTH}");
        // 设备页：一台与音量页共有、一台仅设备页有
        let devices = vec![
            dev("耳机 (两侧都有)", Some(&key_both), Some(50), true, false),
            dev(
                "鼠标 (仅设备页)",
                Some("c:084fb1b9-0000-0000-0000-0000000000b3"),
                Some(70),
                true,
                false,
            ),
        ];
        // 音量页：一台共有、一台仅音量页有
        let outputs = vec![
            audio("扬声器 (两侧都有)", "out-1", Some(CID_BOTH)),
            audio("扬声器 (仅音量页)", "out-2", Some(CID_VOL)),
        ];

        let merged = merge_by_identity(&devices, &outputs, &[]);
        let sel = build_selectable_devices(&merged, &[], &crate::config::Config::default());
        let src = |sub: &str| {
            sel.iter()
                .find(|s| s.name.contains(sub))
                .map(|s| (s.sources.device_page, s.sources.volume_page))
        };

        assert_eq!(src("两侧都有"), Some((true, true)));
        assert_eq!(src("仅设备页"), Some((true, false)));
        assert_eq!(src("仅音量页"), Some((false, true)));
    }

    /// **输入端点必须进并集**（验收判据 2）：只在音量页右键菜单可见的麦克风也要可选。
    #[test]
    fn selectable_devices_include_input_endpoints() {
        const CID: &str = "084fb1b9-0000-0000-0000-0000000000c1";
        // 输入端点：设备页与输出都没有对应条目
        let inputs = vec![audio("麦克风阵列 (USB Audio)", "in-1", Some(CID))];

        let merged = merge_by_identity(&[], &[], &inputs);
        let sel = build_selectable_devices(&merged, &[], &crate::config::Config::default());

        assert_eq!(sel.len(), 1, "输入端点必须在选择器里，实际 {sel:?}");
        assert!(sel[0].sources.volume_page, "来源应标为音量页");
        assert!(!sel[0].sources.device_page, "设备页并没有它");
    }

    /// 无数据设备**必须保留**：选择器不做数据可用性过滤。
    #[test]
    fn selectable_devices_keep_entries_without_any_data() {
        // 键来自降级（`n:`），既无电量也无端点
        let devices = vec![dev("某无线手柄", None, None, false, false)];

        let merged = merge_by_identity(&devices, &[], &[]);
        let sel = build_selectable_devices(&merged, &[], &crate::config::Config::default());

        assert_eq!(sel.len(), 1, "无数据也必须在选择器里出现");
        // 对照：显示侧会因「无数据且未固定」把它滤掉 —— 这正是两者语义不同的证据
        let window = group_taskbar_devices(&devices, &[], &[]);
        assert!(
            window.iter().all(|d| d.name != "某无线手柄"),
            "显示侧过滤掉它（选择器保留）"
        );
    }

    /// **`fallback` 字段的对外契约**：由后端算好返回，前端**零字符串处理**、原样回传。
    ///
    /// ── 为什么让后端算（**不是**因为前端算会得到不同结果）────────────────────
    /// 前端有一个 `simplifyDeviceName`（`common.js:139`），它与 `core_name` 在**两个维度**
    /// 上不等价：① JS 取 `indexOf("(")` 而 Rust 取 `find(" (")`（要求前导空格）；
    /// ② Rust 剥 17 种协议后缀而 JS 不剥。实测 `"耳机 (WH-1000XM5 Stereo)"`：
    /// Rust 得 `WH-1000XM5`、JS 得 `WH-1000XM5 Stereo` ⇒ **两函数确实不等价**。
    ///
    /// ⚠️ **但这条不等价在「选择器 → fallback」路径上不可达**：`m.name` 经 `pick_display_name`
    /// 时**已经过一次 `core_name`**（该函数 `:526`），故它**恒为短名、不含括号**
    /// ⇒ 前端即便拿它去喂 `simplifyDeviceName` 也是**恒等返回**，与后端结果**必然相同**。
    ///
    /// ⇒ 真正的理由只有**判据单一来源**：让「写入配置的 fallback」与「显示侧的判据」
    /// 由**同一个 Rust 函数**产出。将来 `core_name` 的后缀表增删时，两侧一起变；
    /// 若让前端自算，就得在前端**复制一份后缀表**，那才会真正分叉。
    ///
    /// ── 判据的**可证伪性说明**（重要，避免后人误以为测得更严）────────────────
    /// ⛔ 「fallback 必须精确等于 `core_name` 的算式」这条**无法被证伪** ——
    /// 实测把它换成 `DeviceKey::Name(m.name)`（去掉 `core_name`）**单测仍全绿**，
    /// 因为 `core_name` 对短名**幂等**。故**不写**那条假判据（写了只会制造虚假的安全感）。
    /// 本单测只钉**能转红**的部分：往返一致性 + 与显示侧同真同假。
    #[test]
    fn selectable_devices_fallback_round_trips_and_agrees_with_display_side() {
        const CID: &str = "084fb1b9-0000-0000-0000-0000000000f1";
        let key = format!("c:{CID}");
        let devices = vec![dev(
            "耳机 (WH-1000XM5 Stereo)",
            Some(&key),
            None,
            true,
            false,
        )];

        let merged = merge_by_identity(&devices, &[], &[]);
        let sel = build_selectable_devices(&merged, &[], &crate::config::Config::default());
        assert_eq!(sel.len(), 1);

        // ① 显示名已是短名（`pick_display_name` 归一过）+ `core_name` 幂等
        //    —— 这两条是上面「不可达」论证的**可执行依据**，也是本字段设计的立足点。
        assert_eq!(
            sel[0].name, "WH-1000XM5",
            "显示名应已被 core_name 归一（剥掉 ' Stereo'）"
        );
        assert_eq!(
            crate::dedup::core_name(&sel[0].name),
            sel[0].name,
            "core_name 对短名必须幂等"
        );

        // ② 形态正确：必须是名称键（`n:`），因为容器键会随换机/重装驱动变化
        assert!(
            sel[0].fallback.starts_with("n:"),
            "fallback 必须是名称键形态，实际 {}",
            sel[0].fallback
        );

        // ③ ⭐ 往返（**可证伪**）：pin 里存**显示侧算式的产物**，且 `key` 用**另一个容器**
        //    （模拟换机/重装驱动后容器变化）⇒ 精确比较必然失败，只能靠 fallback 命中。
        //    ⛔ 两个坑都会让本判据**失效**，已实测确认：
        //       · 用 `sel[0].fallback` 构造 pin ⇒ **循环论证**（拿输出喂输入再验输出）；
        //       · `pinned.key` 用**同一个**容器键 ⇒ 被 `p.key == key` **短路**，根本走不到 fallback。
        let display_side_fallback = DeviceKey::Name(core_name(&sel[0].name)).encode();
        let pinned = vec![crate::config::PinnedDevice {
            key: "c:084fb1b9-0000-0000-0000-0000000000ff".to_string(), // ← 故意与本次枚举的容器不同
            fallback: Some(display_side_fallback),
            alias: None,
        }];
        let sel2 = build_selectable_devices(&merged, &pinned, &crate::config::Config::default());
        assert!(
            sel2[0].pinned,
            "容器已变、只能靠兜底键命中 ⇒ 必须仍判已固定（若本字段与显示侧分叉，这里会掉）"
        );

        // ④ ⭐ 两侧一致（**可证伪**）：显示侧对同一个 pin 的结论必须相同。
        let window = group_taskbar_devices(&devices, &[], &pinned);
        assert_eq!(
            window[0].pinned, sel2[0].pinned,
            "选择器与显示侧必须同真同假"
        );
    }

    // ── 图标类别（任务栏 widget）────────────────────────────

    #[test]
    fn wireless_24g_kind_maps_to_icon_and_unknown_falls_back() {
        use crate::device::Wireless24gKind;

        assert_eq!(
            audio_kind_from_wireless_kind(Some(Wireless24gKind::Mouse)),
            AudioKind::Pointer
        );
        assert_eq!(
            audio_kind_from_wireless_kind(Some(Wireless24gKind::Keyboard)),
            AudioKind::Keyboard
        );
        assert_eq!(
            audio_kind_from_wireless_kind(Some(Wireless24gKind::Gamepad)),
            AudioKind::Gamepad
        );
        assert_eq!(audio_kind_from_wireless_kind(None), AudioKind::Pointer);
    }

    /// 判据链：无端点 ⇒ Pointer；`扬声器 (…)` ⇒ Speaker；`耳机 (…)` ⇒ Headphones。
    #[test]
    fn audio_kind_maps_prefix_to_icon() {
        assert_eq!(audio_kind_from_endpoint_name(None), AudioKind::Pointer);
        assert_eq!(
            audio_kind_from_endpoint_name(Some("扬声器 (Realtek(R) Audio)")),
            AudioKind::Speaker
        );
        assert_eq!(
            audio_kind_from_endpoint_name(Some("耳机 (WH-1000XM5)")),
            AudioKind::Headphones
        );
    }

    /// ⛔ 判据必须是**前缀**匹配，不是 `contains`：
    /// 「某扬声器叫『耳机支架』」不能被误判成耳机。
    #[test]
    fn audio_kind_uses_prefix_not_contains() {
        assert_eq!(
            audio_kind_from_endpoint_name(Some("扬声器 (耳机支架音频)")),
            AudioKind::Speaker,
            "括号里出现「耳机」不应改变类别（前缀才是判据）"
        );
    }

    /// 英文系统本地化兼容：`Speakers` / `Headphones` 也要正确归类。
    #[test]
    fn audio_kind_handles_english_localization() {
        assert_eq!(
            audio_kind_from_endpoint_name(Some("Headphones (WH-1000XM5)")),
            AudioKind::Headphones
        );
        assert_eq!(
            audio_kind_from_endpoint_name(Some("Speakers (Realtek Audio)")),
            AudioKind::Speaker
        );
        // 无括号的裸名（少见）也要能归类
        assert_eq!(
            audio_kind_from_endpoint_name(Some("Headphones")),
            AudioKind::Headphones
        );
    }
}

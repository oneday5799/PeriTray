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

use serde::{Deserialize, Serialize};
use windows_sys::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_DevNode_PropertyW, CM_Locate_DevNodeW, CM_LOCATE_DEVNODE_NORMAL,
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
}

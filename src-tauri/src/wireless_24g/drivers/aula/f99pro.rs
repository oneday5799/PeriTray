// ── 模块职责 ─────────────────────────────────────────────
// AULA F99 Pro/X99 Pro 无线协议（Report ID 0x13 报文族，20 字节）。
// 协议事实转录自 aula-x99-pro-driver-linux 的 PROTOCOL.md；
// 仅支持 2.4G 无线模式（有线固件为另一套协议且电量 N/A）。
//
// Windows 兼容性：暂不支持。
// write()（WriteFile → 中断 OUT）在 vendor 集合（0xFF02）上成功（无 ACCESS_DENIED），
// 但设备不回复中断 IN 响应（500ms 超时）。C hidapi 与 Rust hidapi 机制完全一致，
// 排除 API 路径差异。可能原因：Windows HID 类驱动多集合路由 / 固件差异 / 需初始化序列。
// 详见 Wiki「F99 Pro 排障过程」页。

use crate::wireless_24g::hid_link::HidLink;

pub const VID: u16 = 0x3554;
pub const PID: u16 = 0xFA09;

/// 读取电量百分比。
/// Windows 上暂不支持：write 可达但设备不回复。
pub fn read_battery_percent(_link: &HidLink) -> Result<Option<i32>, String> {
    Err("F99 Pro 在 Windows 上暂不支持：write 可达但设备不回复中断 IN 响应".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vid_pid_match_known_device() {
        assert_eq!(VID, 0x3554);
        assert_eq!(PID, 0xFA09);
    }
}

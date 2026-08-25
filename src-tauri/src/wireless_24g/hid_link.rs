// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 接收器 HID 传输层：基于 hidapi 的薄封装，负责枚举设备集合、
// 收发 Feature Report。协议无关，报文的组包/解析由各品牌驱动自行实现。

use std::ffi::CString;
use std::time::Duration;

use hidapi::HidApi;

/// Razer 风格报文定长（90 字节）
pub const REPORT_LEN: usize = 90;
/// Windows 下 Feature Report 需带 1 字节 Report ID 前缀（无 ID 时补 0x00）
const HID_BUF_LEN: usize = REPORT_LEN + 1;

/// HID 会话：复用同一 HidApi 实例完成枚举与收发，避免反复初始化
pub struct HidLink {
    api: HidApi,
}

/// 候选集合路径及其拓扑信息（枚举排序与诊断日志共用）
pub struct HidPath {
    /// 设备接口路径（open_path 用）
    pub path: String,
    /// UsagePage（厂商自定义页 ≥0xFF00 优先）
    pub usage_page: u16,
    /// Usage
    pub usage: u16,
    /// USB 接口号（未验证设备的接口差异定位）
    pub interface_number: i32,
}

impl HidLink {
    /// 初始化 HID 会话
    pub fn new() -> Result<Self, String> {
        let api = HidApi::new().map_err(|e| format!("HID 初始化失败: {}", e))?;
        Ok(Self { api })
    }

    /// 枚举匹配 VID/PID 的所有 HID 集合路径。
    /// 键盘等标准集合会被系统封锁写入；厂商自定义集合（UsagePage >= 0xFF00）
    /// 是常规控制通道，而部分接收器（如 Orochi V2）没有厂商集合、
    /// 控制报文实际由鼠标集合（UsagePage 0x0001 / Usage 0x0002）应答，
    /// 故按「厂商 → 鼠标 → 其余」排序逐一试探，以回显校验确认有效通道。
    pub fn enumerate_paths(&self, vid: u16, pid: u16) -> Result<Vec<HidPath>, String> {
        let mut vendor = vec![];
        let mut mice = vec![];
        let mut others = vec![];
        for dev in self.api.device_list() {
            if dev.vendor_id() == vid && dev.product_id() == pid {
                let hp = HidPath {
                    path: dev.path().to_string_lossy().into_owned(),
                    usage_page: dev.usage_page(),
                    usage: dev.usage(),
                    interface_number: dev.interface_number(),
                };
                match (hp.usage_page, hp.usage) {
                    (page, _) if page >= 0xFF00 => vendor.push(hp),
                    (0x0001, 0x0002) => mice.push(hp),
                    _ => others.push(hp),
                }
            }
        }
        if vendor.is_empty() && mice.is_empty() && others.is_empty() {
            return Err(format!("未找到 {:04X}:{:04X} 对应的 HID 设备", vid, pid));
        }
        vendor.extend(mice);
        vendor.extend(others);
        Ok(vendor)
    }

    /// 打开指定路径并完成一次「发送请求 → 等待 → 取回响应」。
    /// 返回 90 字节响应体（已剥离 Report ID 前缀）。
    pub fn exchange(
        &self,
        path: &str,
        request: &[u8; REPORT_LEN],
        wait_ms: u64,
    ) -> Result<[u8; REPORT_LEN], String> {
        let path_c = CString::new(path).map_err(|_| "设备路径含非法字符".to_string())?;
        let dev = self
            .api
            .open_path(&path_c)
            .map_err(|e| format!("打开设备失败: {}", e))?;

        let mut out = [0u8; HID_BUF_LEN];
        out[0] = 0x00;
        out[1..].copy_from_slice(request);
        dev.send_feature_report(&out)
            .map_err(|e| format!("发送 Feature Report 失败: {}", e))?;

        // 无线接收器响应较慢，按设备参数等待后再取回
        std::thread::sleep(Duration::from_millis(wait_ms));

        let mut buf = [0u8; HID_BUF_LEN];
        buf[0] = 0x00;
        let n = dev
            .get_feature_report(&mut buf)
            .map_err(|e| format!("读取 Feature Report 失败: {}", e))?;
        if n < HID_BUF_LEN {
            return Err(format!("响应长度不足: {} / {}", n, HID_BUF_LEN));
        }
        let mut resp = [0u8; REPORT_LEN];
        resp.copy_from_slice(&buf[1..]);
        Ok(resp)
    }
}

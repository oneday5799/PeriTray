// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 接收器 HID 传输层：基于 hidapi 的薄封装，负责枚举设备集合、
// 收发 Feature Report。协议无关，报文的组包/解析由各品牌驱动自行实现。

use std::ffi::CString;
use std::time::Duration;

use hidapi::HidApi;

use crate::device_identity;
use crate::standard_log;

/// Razer 风格报文定长（90 字节）
pub const REPORT_LEN: usize = 90;
/// Windows 下 Feature Report 需带 1 字节 Report ID 前缀（无 ID 时补 0x00）
const HID_BUF_LEN: usize = REPORT_LEN + 1;

/// HID 会话：复用同一 HidApi 实例完成枚举与收发，避免反复初始化
pub struct HidLink {
    api: HidApi,
}

/// 候选集合路径及其拓扑信息（枚举排序与诊断日志共用）
#[derive(Debug)]
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

/// hidapi 路径 → 容器 GUID（分域过滤的实际解析入口）。
///
/// 两步都是纯 Windows API 调用，无 IPC：`devnode_from_hidapi_path` 是字符串变换，
/// `container_of_instance` 是 2 次 cfgmgr32 调用。
fn container_of_hidapi_path(path: &str) -> Option<String> {
    device_identity::devnode_from_hidapi_path(path)
        .as_deref()
        .and_then(device_identity::container_of_instance)
}

/// 候选集合的**设备分域**过滤。
///
/// `scope` 为 `None`（设备本身拿不到容器）时**一律放行** —— 退化为历史行为，
/// 「解析不到」不该变成「没有电量」。
///
/// `scope` 为 `Some` 时只保留解析出的容器与之一致的集合。
/// ⚠️ 解析失败（`None`）**不放行，也不回退到全部集合**：
/// 回退会让分域静默失效，重新变成「读到另一台的值」——
/// 而「暂时没电量」至少是诚实的。这条取舍由 `enumerate_paths` 的返回 `Err` 兜住。
///
/// `resolve` 以参数注入，便于用假解析器做单测（真实解析需要实机 HID 设备）。
fn filter_by_scope(
    paths: Vec<HidPath>,
    scope: Option<&str>,
    resolve: &dyn Fn(&str) -> Option<String>,
) -> Vec<HidPath> {
    let Some(scope) = scope else {
        return paths;
    };
    paths
        .into_iter()
        .filter(|p| resolve(&p.path).as_deref() == Some(scope))
        .collect()
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
    ///
    /// `scope` 是**设备容器 GUID**（规范化小写无花括号）：同型号两台接收器的
    /// HID 集合全部同型号同 PID，只能靠容器区分，否则会读到另一台的值。
    /// 传 `None` 表示调用方拿不到容器 ⇒ 不做分域（退化为历史行为）。
    pub fn enumerate_paths(
        &self,
        vid: u16,
        pid: u16,
        scope: Option<&str>,
    ) -> Result<Vec<HidPath>, String> {
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

        let found = vendor.len();
        let vendor = filter_by_scope(vendor, scope, &container_of_hidapi_path);
        if vendor.is_empty() {
            // 不回退：回退会让分域静默失效（详见 filter_by_scope 的取舍说明）
            return Err(format!(
                "{:04X}:{:04X} 枚举到 {found} 个 HID 集合，但没有一个属于容器 {} —— 不回退（回退会让分域静默失效）",
                vid,
                pid,
                scope.unwrap_or("?")
            ));
        }
        if vendor.len() != found {
            standard_log!(
                "[24g] {:04X}:{:04X} 按容器分域: {} → {} 个集合",
                vid,
                pid,
                found,
                vendor.len()
            );
        }
        Ok(vendor)
    }

    /// 打开指定路径返回原始句柄——供非 Feature Report 模型的驱动
    /// （如罗技 HID++ 的 output/input report 收发）自行组织读写
    pub fn open_path_handle(&self, path: &str) -> Result<hidapi::HidDevice, String> {
        let path_c = CString::new(path).map_err(|_| "设备路径含非法字符".to_string())?;
        self.api
            .open_path(&path_c)
            .map_err(|e| format!("打开设备失败: {}", e))
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

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str) -> HidPath {
        HidPath {
            path: p.to_string(),
            usage_page: 0x0001,
            usage: 0x0002,
            interface_number: 0,
        }
    }

    /// 假解析器：按路径里是否含 `alpha` / `beta` 归到两个容器。
    /// 用于在不接实机的前提下验证过滤语义。
    fn fake_resolver(p: &str) -> Option<String> {
        if p.contains("alpha") {
            Some("c-alpha".to_string())
        } else if p.contains("beta") {
            Some("c-beta".to_string())
        } else {
            None
        }
    }

    fn names(paths: &[HidPath]) -> Vec<&str> {
        paths.iter().map(|p| p.path.as_str()).collect()
    }

    /// 核心判据：分域后只剩本设备容器的集合 —— 同型号另一台的集合必须被剔除。
    #[test]
    fn scope_keeps_only_own_container() {
        let all = vec![path("dev-alpha-1"), path("dev-beta-1"), path("dev-alpha-2")];
        let kept = filter_by_scope(all, Some("c-alpha"), &fake_resolver);
        assert_eq!(names(&kept), vec!["dev-alpha-1", "dev-alpha-2"]);
    }

    /// `scope` 为 `None` ⇒ 一律放行（拿不到容器时退化为历史行为，不丢电量）。
    #[test]
    fn no_scope_keeps_everything() {
        let all = vec![path("dev-alpha-1"), path("dev-beta-1"), path("dev-unknown")];
        let kept = filter_by_scope(all, None, &fake_resolver);
        assert_eq!(kept.len(), 3, "无 scope 时不得过滤：{kept:?}");
    }

    /// ⛔ 关键取舍：解析不出的集合在**有 scope 时不得放行**。
    ///
    /// 若这里放行，同型号另一台的集合就会漏进来 ⇒ 重新变成「读到另一台的值」。
    /// 可证伪：把 `filter` 改成 `resolve(&p.path).as_deref() != Some(scope)` 之外的
    /// 宽松写法（例如解析失败即放行）本用例必转红。
    #[test]
    fn unresolvable_paths_are_dropped_when_scope_present() {
        let all = vec![path("dev-alpha-1"), path("dev-unknown")];
        let kept = filter_by_scope(all, Some("c-alpha"), &fake_resolver);
        assert_eq!(names(&kept), vec!["dev-alpha-1"], "解析不出的必须剔除");
    }

    /// 空结果必须保持为空 —— `enumerate_paths` 依赖这一点返回 Err 而**不回退**。
    /// 若这里被改成「空则返回全部」，分域会静默失效。
    #[test]
    fn empty_result_stays_empty_so_caller_can_refuse_to_fall_back() {
        let all = vec![path("dev-beta-1"), path("dev-beta-2")];
        let kept = filter_by_scope(all, Some("c-alpha"), &fake_resolver);
        assert!(kept.is_empty(), "不得回退到全部集合：{kept:?}");
    }
}

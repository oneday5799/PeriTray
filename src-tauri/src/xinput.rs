// ── 模块职责 ─────────────────────────────────────────────
// XInput 电池状态读取：动态加载 xinput1_4.dll，通过 XInputGetBatteryInformation
// 获取 Xbox 360 / Xbox One 手柄的电池等级。
// XInput 按控制器索引（0-3）寻址，不暴露 VID/PID，仅提供4档电量
// （空/低/中/满），本模块将其映射为近似百分比。
//
// 用途：作为 2.4G 电量查询的兜底路径——当 HID 驱动不识别设备但设备以
// Xbox 360 Controller 身份出现在 XInput 中时（如 Flydigi Vader 4 Pro
// 的 XInput 模式），仍可读取粗粒度电量。

use std::os::raw::c_void;

// ── XInput FFI ───────────────────────────────────────────

const XINPUT_DEVTYPE_GAMEPAD: u32 = 0x01;
const XINPUT_STATE_CONNECTED: u32 = 0x00;

const BATTERY_TYPE_ALKALINE: u8 = 0x02;
const BATTERY_TYPE_NIMH: u8 = 0x03;

const BATTERY_LEVEL_EMPTY: u8 = 0x00;
const BATTERY_LEVEL_LOW: u8 = 0x01;
const BATTERY_LEVEL_MEDIUM: u8 = 0x02;
const BATTERY_LEVEL_FULL: u8 = 0x03;

/// 20 字节，与 Win32 XINPUT_BATTERY_INFORMATION 布局一致
#[repr(C)]
struct XinputBatteryInformation {
    battery_type: u8,
    battery_level: u8,
}

type XinputGetBatteryInfoFn =
    unsafe extern "system" fn(u32, u32, *mut XinputBatteryInformation) -> u32;
type RawXinputFn = unsafe extern "system" fn() -> u32;

// ── 动态加载 ─────────────────────────────────────────────

/// 缓存 XInput DLL 模块句柄（加载一次，进程生命周期常驻）
fn xinput_module() -> *mut c_void {
    static MODULE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *MODULE.get_or_init(|| unsafe {
        // 优先 xinput1_4.dll（Win8+），xinput9_1_0.dll 为 Vista+ 备选
        let m = crate::process::load_library(b"xinput1_4.dll\0");
        if !m.is_null() {
            return m as usize;
        }
        crate::process::load_library(b"xinput9_1_0.dll\0") as usize
    }) as *mut c_void
}

unsafe fn xinput_proc(name: &[u8]) -> Option<RawXinputFn> {
    let ptr = crate::process::get_proc_address(xinput_module(), name);
    if ptr.is_null() {
        None
    } else {
        Some(std::mem::transmute(ptr))
    }
}

/// 获取 XInputGetBatteryInformation 函数指针
fn get_xinput_battery_info() -> Option<XinputGetBatteryInfoFn> {
    static FN: std::sync::OnceLock<Option<XinputGetBatteryInfoFn>> = std::sync::OnceLock::new();
    *FN.get_or_init(|| unsafe {
        xinput_proc(b"XInputGetBatteryInformation\0").map(|f| std::mem::transmute(f))
    })
}

// ── 电量等级 → 百分比映射 ────────────────────────────────

/// XInput 电量档位映射为近似百分比
fn level_to_percent(level: u8) -> Option<i32> {
    match level {
        BATTERY_LEVEL_EMPTY => Some(5),
        BATTERY_LEVEL_LOW => Some(25),
        BATTERY_LEVEL_MEDIUM => Some(60),
        BATTERY_LEVEL_FULL => Some(100),
        _ => None,
    }
}

/// 检查电池类型：有线供电不报百分比
fn battery_type_has_percentage(t: u8) -> bool {
    matches!(t, BATTERY_TYPE_ALKALINE | BATTERY_TYPE_NIMH)
}

// ── 对外接口 ─────────────────────────────────────────────

/// 读取指定控制器索引的电池百分比。
/// Ok(Some(%))=无线手柄有电；Ok(None)=有线供电（无百分比）；Err=不可用
fn read_battery(controller_index: u32) -> Result<Option<i32>, String> {
    if controller_index > 3 {
        return Err("控制器索引超出范围（XInput 仅支持 0-3）".into());
    }

    let get_battery = get_xinput_battery_info()
        .ok_or_else(|| "XInput DLL 不可用（xinput1_4.dll / xinput9_1_0.dll 未找到）".to_string())?;

    let mut battery_info = XinputBatteryInformation {
        battery_type: 0,
        battery_level: 0,
    };
    let hr = unsafe { get_battery(controller_index, XINPUT_DEVTYPE_GAMEPAD, &mut battery_info) };

    if hr != XINPUT_STATE_CONNECTED {
        return Err(format!("控制器 {} 未连接", controller_index));
    }

    if !battery_type_has_percentage(battery_info.battery_type) {
        return Ok(None); // 有线供电或未知类型，不报百分比
    }

    level_to_percent(battery_info.battery_level)
        .map(Some)
        .ok_or_else(|| format!("未知电量等级: {}", battery_info.battery_level))
}

/// 扫描所有 XInput 控制器索引，返回第一个可用的电池百分比。
/// 用于不区分具体 VID/PID 的兜底查询（XInput 不暴露设备身份）
pub fn scan_battery() -> Option<i32> {
    for i in 0..=3 {
        match read_battery(i) {
            Ok(Some(pct)) => {
                crate::process::append_verbose_log(&format!("[xinput] 索引 {} 电量 {}%", i, pct));
                return Some(pct);
            }
            Ok(None) => {
                crate::process::append_verbose_log(&format!(
                    "[xinput] 索引 {} 有线供电，无百分比",
                    i
                ));
            }
            Err(e) => {
                crate::process::append_verbose_log(&format!("[xinput] 索引 {} 不可用: {}", i, e));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn level_mapping() {
        assert_eq!(level_to_percent(BATTERY_LEVEL_EMPTY), Some(5));
        assert_eq!(level_to_percent(BATTERY_LEVEL_LOW), Some(25));
        assert_eq!(level_to_percent(BATTERY_LEVEL_MEDIUM), Some(60));
        assert_eq!(level_to_percent(BATTERY_LEVEL_FULL), Some(100));
        assert_eq!(level_to_percent(99), None);
    }

    #[test]
    fn battery_type_check() {
        assert!(battery_type_has_percentage(BATTERY_TYPE_ALKALINE));
        assert!(battery_type_has_percentage(BATTERY_TYPE_NIMH));
        assert!(!battery_type_has_percentage(0x00)); // DISCONNECTED
        assert!(!battery_type_has_percentage(0x01)); // WIRED
    }

    #[test]
    fn index_out_of_range() {
        assert!(read_battery(4).is_err());
        assert!(read_battery(100).is_err());
    }
}

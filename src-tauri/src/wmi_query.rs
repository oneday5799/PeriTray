use regex::Regex;
use serde::Deserialize;
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock};
use wmi::WMIConnection;

use crate::bluetooth::find_paired_bluetooth_devices;
use crate::classify::{
    classify_bluetooth, classify_device, is_bt_service, is_generic_hid, is_system_device,
    is_wireless_24g_by_vid_pid,
};
use crate::config;
use crate::dedup::{core_name, try_insert};
use crate::device::{DevType, Device};
use crate::device_data;

/// 蓝牙设备状态字符串：WMI 查询构造与托盘图标判断共用，避免字面量散落
pub const BT_STATUS_CONNECTED: &str = "已连接";
pub const BT_STATUS_PAIRED: &str = "已配对";
static CACHED_REGEX: OnceLock<Mutex<Option<(String, Arc<Regex>)>>> = OnceLock::new();

fn get_cached_regex(pattern: &str) -> Option<Arc<Regex>> {
    let cache = CACHED_REGEX.get_or_init(|| Mutex::new(None));
    let mut guard = crate::state::lock_unpoisoned(cache);
    if let Some((ref cached_pat, ref re)) = *guard {
        if cached_pat == pattern {
            return Some(Arc::clone(re));
        }
    }
    let re = Arc::new(Regex::new(&format!("(?i)({})", pattern)).ok()?);
    *guard = Some((pattern.to_string(), Arc::clone(&re)));
    Some(re)
}

/// 从 WMI 行中提取字符串字段
fn wmi_str(row: &HashMap<String, wmi::Variant>, key: &str) -> String {
    match row.get(key) {
        Some(wmi::Variant::String(s)) => s.clone(),
        _ => String::new(),
    }
}

#[derive(Deserialize, Debug)]
#[serde(rename = "Win32_Battery")]
struct BatteryDevice {
    name: Option<String>,
    status: Option<String>,
    estimated_charge_remaining: Option<i32>,
}

/// fresh_24g=true 时强制现查 2.4G 接收器电量（设备列表手动刷新入口），否则走缓存。
/// Err 表示主查询通道不可信（COM/WMI 连接失败或 PnP 主查询失败），
/// 调用方应保留既有数据而非应用残缺结果。
pub fn query_devices(fresh_24g: bool) -> Result<Vec<Device>, String> {
    let mut all = vec![];
    let mut seen = HashSet::new();
    let mut cn_index: HashMap<String, Vec<usize>> = HashMap::new();

    crate::device_data::reload_device_data();

    let (filter_enabled, dedup_enabled, filter_regex_str) =
        config::with_config(|c| (c.filter_enabled, c.dedup_devices, c.filter_regex.clone()));

    let com = unsafe { wmi::COMLibrary::assume_initialized() };
    let con = match WMIConnection::new(com) {
        Ok(c) => c,
        Err(_) => {
            crate::process::append_log("[wmi] WMIConnection::new failed");
            return Err("wmi connection failed".to_string());
        }
    };

    let mut bt_names = HashSet::new();
    let mut pnp_24g_pairs = vec![];

    if let Err(e) = query_pnp_devices(
        &con,
        dedup_enabled,
        &mut seen,
        &mut all,
        &mut cn_index,
        &mut pnp_24g_pairs,
    ) {
        crate::process::append_log(&format!("[wmi] {}", e));
        return Err(e);
    }
    query_bt_devices(
        dedup_enabled,
        &mut seen,
        &mut all,
        &mut bt_names,
        &mut cn_index,
    );
    query_battery_devices(&con, dedup_enabled, &mut seen, &mut all, &mut cn_index);

    // Apply user-defined regex filter
    if filter_enabled && !filter_regex_str.is_empty() {
        if let Some(re) = get_cached_regex(&filter_regex_str) {
            all.retain(|d| !re.is_match(&d.name));
        }
    }

    // Temporarily hide status for devices not detected by WinRT Bluetooth API
    for d in &mut all {
        if d.is_bluetooth && !bt_names.contains(&core_name(&d.name)) {
            d.status.clear();
        }
    }

    // 实验性：2.4G 接收器电量并入列表（读缓存即时返回，手动刷新时现查）
    fill_24g_battery(&mut all, pnp_24g_pairs, fresh_24g);

    crate::process::append_log(&format!("[wmi] query_devices: {} devices found", all.len()));
    Ok(all)
}

fn query_pnp_devices(
    con: &WMIConnection,
    dedup: bool,
    seen: &mut HashSet<String>,
    all: &mut Vec<Device>,
    cn_index: &mut HashMap<String, Vec<usize>>,
    p24g_pairs: &mut Vec<(String, String)>,
) -> Result<(), String> {
    const PNPCLASS_WHITELIST: &[&str] = &[
        "AudioEndpoint",
        "Bluetooth",
        "HIDClass",
        "Keyboard",
        "MEDIA",
        "Mouse",
        "Monitor",
    ];

    let rows = match con.raw_query::<HashMap<String, wmi::Variant>>(
        "SELECT Name, Status, PNPDeviceID, Caption, PNPClass, ConfigManagerErrorCode FROM Win32_PnPEntity",
    ) {
        Ok(r) => r,
        Err(e) => {
            let msg = format!("pnp query failed: {}", e);
            crate::process::append_log(&format!("[wmi] {}", msg));
            return Err(msg);
        }
    };

    for row in rows {
        let n = match row.get("Name") {
            Some(wmi::Variant::String(s)) => s.clone(),
            _ => continue,
        };
        let devid = wmi_str(&row, "PNPDeviceID");
        let cap = wmi_str(&row, "Caption");
        let pnp = wmi_str(&row, "PNPClass");
        let status_str = wmi_str(&row, "Status");

        if !PNPCLASS_WHITELIST
            .iter()
            .any(|c| pnp.eq_ignore_ascii_case(c))
        {
            continue;
        }

        let u = devid.to_uppercase();

        let err_val = row.get("ConfigManagerErrorCode").and_then(|v| match v {
            wmi::Variant::I2(v) => Some(*v as i64),
            wmi::Variant::I4(v) => Some(*v as i64),
            wmi::Variant::UI2(v) => Some(*v as i64),
            wmi::Variant::UI4(v) => Some(*v as i64),
            wmi::Variant::String(s) => s.parse::<i64>().ok(),
            wmi::Variant::Bool(v) => Some(if *v { 0 } else { 1 }),
            _ => None,
        });
        let connected = match err_val {
            Some(code) => code == 0,
            None => status_str == "OK",
        };
        let s = if connected {
            BT_STATUS_CONNECTED
        } else {
            BT_STATUS_PAIRED
        };
        if n.is_empty() {
            continue;
        }

        if pnp.eq_ignore_ascii_case("Bluetooth") && is_bt_service(&u) {
            continue;
        }
        if pnp.eq_ignore_ascii_case("HIDClass") && is_generic_hid(&u) {
            continue;
        }
        if is_system_device(&u) {
            continue;
        }

        let dt = classify_device(&n, &pnp, &u, &cap);
        let is_24g = is_wireless_24g_by_vid_pid(&u);
        let vid_pid_24g = if is_24g {
            device_data::extract_vid_pid(&u)
        } else {
            None
        };
        let display_name = vid_pid_24g
            .as_ref()
            .and_then(|(vid, pid)| device_data::get_device_name(vid, pid));
        // 收集 2.4G 设备的 VID/PID，供电量缓存模块使用
        if let Some(pair) = vid_pid_24g {
            p24g_pairs.push(pair);
        }
        try_insert(
            &n,
            display_name.as_deref(),
            dt,
            s,
            None,
            None,
            false,
            is_24g,
            dedup,
            seen,
            all,
            cn_index,
        );
    }
    Ok(())
}

fn query_bt_devices(
    dedup: bool,
    seen: &mut HashSet<String>,
    all: &mut Vec<Device>,
    bt_names: &mut HashSet<String>,
    cn_index: &mut HashMap<String, Vec<usize>>,
) {
    let btc_devices = match find_paired_bluetooth_devices() {
        Ok(d) => d,
        Err(_) => return,
    };

    for (name, connected, battery, device_id) in btc_devices {
        if name.is_empty() {
            continue;
        }
        let dt = match classify_bluetooth(&name) {
            Some(dt) => dt,
            None => continue,
        };
        let s = if connected {
            BT_STATUS_CONNECTED
        } else {
            BT_STATUS_PAIRED
        };
        let cn = core_name(&name);
        bt_names.insert(cn.clone());
        if let Some(existing) = all
            .iter_mut()
            .find(|d| core_name(&d.name) == cn && d.is_bluetooth)
        {
            existing.status = s.to_string();
            if battery.is_some() {
                existing.battery = battery.map(|b| b as i32);
            }
            if existing.device_id.is_none() {
                existing.device_id = Some(device_id);
            }
        } else {
            try_insert(
                &name,
                None,
                dt,
                s,
                battery.map(|b| b as i32),
                Some(device_id),
                true,
                false,
                dedup,
                seen,
                all,
                cn_index,
            );
        }
    }
}

fn query_battery_devices(
    con: &WMIConnection,
    dedup: bool,
    seen: &mut HashSet<String>,
    all: &mut Vec<Device>,
    cn_index: &mut HashMap<String, Vec<usize>>,
) {
    if let Ok(r) = con.query::<BatteryDevice>() {
        for d in r {
            let (n, s) = (d.name.unwrap_or_default(), d.status.unwrap_or_default());
            if n.is_empty() {
                continue;
            }
            let cn = core_name(&n);
            if dedup && seen.contains(&format!("{}:usb", cn)) {
                continue;
            }
            seen.insert(format!("{}:usb", cn));
            let idx = all.len();
            all.push(Device {
                name: cn.clone(),
                dt: DevType::Battery,
                status: s,
                battery: d.estimated_charge_remaining,
                device_id: None,
                is_bluetooth: false,
                is_wireless_24g: false,
            });
            cn_index.entry(cn).or_default().push(idx);
        }
    }
}

/// 实验性：将 2.4G 接收器缓存电量填入设备列表（开关关闭时整体旁路）。
/// 默认只读缓存即时返回，实际 HID 查询由 wireless_24g 后台线程完成；
/// fresh=true 时同步现查（手动刷新按钮，耗时约 0.5~2 秒）。
fn fill_24g_battery(all: &mut [Device], pairs: Vec<(String, String)>, fresh: bool) {
    let supported: Vec<_> = pairs
        .into_iter()
        .filter(|(v, p)| crate::wireless_24g::supported(v, p))
        .collect();
    let enabled = config::with_config(|c| c.enable_24g_battery);
    if !enabled || supported.is_empty() {
        return;
    }

    let snap = crate::wireless_24g::snapshot(supported, fresh);
    // 缓存键 → 数据库显示名，用于把电量对回列表条目（同名设备共享同一接收器型号）
    let filled: Vec<(String, Option<i32>)> = snap
        .iter()
        .filter_map(|((v, p), lvl)| device_data::get_device_name(v, p).map(|n| (n, *lvl)))
        .collect();
    for d in all.iter_mut() {
        if !d.is_wireless_24g || d.battery.is_some() {
            continue;
        }
        if let Some((_, lvl)) = filled.iter().find(|(n, _)| *n == d.name) {
            d.battery = *lvl;
        }
    }
}

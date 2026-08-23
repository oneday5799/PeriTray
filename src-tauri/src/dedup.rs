use std::collections::{HashMap, HashSet};
use crate::device::{Device, DevType};

/// 取设备名括号内核心名并剥离蓝牙协议后缀（Hands-Free/Stereo/LE/Audio 等），用于重名合并。
/// 注意：与 tray::simplify_device_name 语义不同——本函数会剥后缀返回 String，两者勿互相替换。
pub fn core_name(n: &str) -> String {
    let base = if let Some(i) = n.find(" (") {
        if let Some(j) = n.rfind(')') {
            if j > i + 2 { &n[i + 2..j] } else { n }
        } else { n }
    } else { n };
    for suffix in &[
        " Hands-Free AG",
        " Hands-Free HF",
        " Hands-Free",
        " Handsfree",
        " A2DP SNK",
        " A2DP SRC",
        " Stereo",
        " LE",
        " Low Energy",
        " Audio",
        " HFP",
        " AG",
        " SNK",
        " SRC",
        " Avrcp 传输",
        " 音频网关服务",
    ] {
        if let Some(pos) = base.strip_suffix(suffix) {
            return pos.to_string();
        }
    }
    base.to_string()
}

pub fn try_insert(
    name: &str,
    display_name: Option<&str>,
    dt: DevType,
    status: &str,
    battery: Option<i32>,
    device_id: Option<String>,
    is_bluetooth: bool,
    is_wireless_24g: bool,
    dedup: bool,
    seen: &mut HashSet<String>,
    devices: &mut Vec<Device>,
    cn_index: &mut HashMap<String, Vec<usize>>,
) {
    let effective_name = display_name.unwrap_or(name);
    let cn = if display_name.is_some() {
        effective_name.to_string()
    } else {
        core_name(name)
    };
    let has_conn_type = is_bluetooth || is_wireless_24g;

    if dedup && !has_conn_type {
        if let Some(indices) = cn_index.get(&cn) {
            if indices.iter().any(|&i| {
                let d = &devices[i];
                let d_cn = core_name(&d.name);
                d_cn == cn && (d.is_bluetooth || d.is_wireless_24g)
            }) {
                return;
            }
        }
    }

    if dedup && has_conn_type {
        if let Some(indices) = cn_index.get(&cn) {
            if let Some(&pos) = indices.iter().find(|&&i| {
                let d = &devices[i];
                let d_cn = core_name(&d.name);
                d_cn == cn && !d.is_bluetooth && !d.is_wireless_24g
            }) {
                devices.remove(pos);
                rebuild_cn_index(cn_index, devices);
            }
        }
    }

    let conn_tag = if is_bluetooth { "bt" } else if is_wireless_24g { "24g" } else { "usb" };
    let dedup_key = format!("{}:{}", cn, conn_tag);
    if dedup && !seen.insert(dedup_key) {
        if let Some(indices) = cn_index.get(&cn) {
            if let Some(&pos) = indices.iter().find(|&&i| {
                let d = &devices[i];
                let d_cn = core_name(&d.name);
                let econn = if d.is_bluetooth { "bt" } else if d.is_wireless_24g { "24g" } else { "usb" };
                d_cn == cn && econn == conn_tag
            }) {
                let existing = &mut devices[pos];
                existing.status = status.to_string();
                if existing.device_id.is_none() {
                    existing.device_id = device_id;
                }
                existing.is_bluetooth = existing.is_bluetooth || is_bluetooth;
                existing.is_wireless_24g = existing.is_wireless_24g || is_wireless_24g;
            }
        }
        return;
    }
    let idx = devices.len();
    devices.push(Device {
        name: cn.clone(),
        dt,
        status: status.to_string(),
        battery,
        device_id,
        is_bluetooth,
        is_wireless_24g,
    });
    cn_index.entry(cn).or_default().push(idx);
}

fn rebuild_cn_index(cn_index: &mut HashMap<String, Vec<usize>>, devices: &[Device]) {
    cn_index.clear();
    for (i, d) in devices.iter().enumerate() {
        let cn = core_name(&d.name);
        cn_index.entry(cn).or_default().push(i);
    }
}

use crate::config;
use crate::device::DevType;
use crate::device_data;

pub(crate) fn classify_device(name: &str, pnp_class: &str, pnp_id: &str, caption: &str) -> DevType {
    let lower_combined = format!("{} {}", name, caption).to_lowercase();

    let result = classify_device_inner(&lower_combined, pnp_class, pnp_id);
    crate::process::append_verbose_log(&format!(
        "[classify] classify_device: {} -> {:?} (pnp_class={}, pnp_id={})",
        name, result, pnp_class, pnp_id
    ));
    result
}

fn classify_device_inner(lower_combined: &str, pnp_class: &str, pnp_id: &str) -> DevType {
    // 按 VID/PID 检测 2.4G 无线设备并路由到对应类型
    if pnp_id.starts_with("USB\\") && is_wireless_24g_by_vid_pid(pnp_id) {
        if let Some((vid, pid)) = device_data::extract_vid_pid(pnp_id) {
            let dev_type = device_data::get_device_type(&vid, &pid);
            return match dev_type.as_str() {
                "mouse" | "keyboard" => DevType::Usb,
                "audio" => DevType::Audio,
                _ => DevType::Other,
            };
        }
        return DevType::Other;
    }

    if pnp_class.eq_ignore_ascii_case("AudioEndpoint") || pnp_class.eq_ignore_ascii_case("MEDIA") {
        return DevType::Audio;
    }
    if pnp_class.eq_ignore_ascii_case("Keyboard") || pnp_class.eq_ignore_ascii_case("Mouse") {
        return DevType::Usb;
    }
    if pnp_class.eq_ignore_ascii_case("Monitor") {
        return DevType::Monitor;
    }
    if pnp_class.eq_ignore_ascii_case("Bluetooth")
        || pnp_id.starts_with("BTHENUM\\")
        || pnp_id.starts_with("SWD\\")
    {
        if is_audio(lower_combined) {
            return DevType::Audio;
        }
        if match_usb_keyword(lower_combined) {
            return DevType::Usb;
        }
        return DevType::Other;
    }
    if pnp_class.eq_ignore_ascii_case("HIDClass") {
        if is_audio(lower_combined) {
            return DevType::Audio;
        }
        if match_usb_keyword(lower_combined) {
            return DevType::Usb;
        }
        return DevType::Other;
    }
    if pnp_id.starts_with("USB\\") && match_usb_keyword(lower_combined) {
        return DevType::Usb;
    }
    DevType::Other
}

pub(crate) fn classify_bluetooth(name: &str) -> Option<DevType> {
    let result = classify_bluetooth_inner(name);
    crate::process::append_verbose_log(&format!(
        "[classify] classify_bluetooth: {} -> {:?}",
        name, result
    ));
    result
}

fn classify_bluetooth_inner(name: &str) -> Option<DevType> {
    // MAC-address-only BLE devices (e.g. "Bluetooth e0:cc:f8:7f:d9:eb")
    if name.starts_with("Bluetooth ") && name.len() == 27 && name.as_bytes()[12] == b':' {
        if config::with_config(|c| c.show_unnamed_bt) {
            return Some(DevType::Other);
        }
        return None;
    }
    let lower = name.to_lowercase();
    if is_audio(&lower) {
        return Some(DevType::Audio);
    }
    if match_usb_keyword(&lower) {
        return Some(DevType::Usb);
    }
    Some(DevType::Other)
}

pub(crate) fn is_wireless_24g_by_vid_pid(pnp_id: &str) -> bool {
    match device_data::extract_vid_pid(pnp_id) {
        Some((vid, pid)) => device_data::is_wireless_24g(&vid, &pid),
        None => false,
    }
}

fn is_audio(lower: &str) -> bool {
    [
        "headphone",
        "headset",
        "earphone",
        "earbuds",
        "speaker",
        "耳机",
        "音箱",
        "扬声器",
        "音响",
        "airpods",
        "hifi",
        "dac",
        "amp",
        "glasses",
        "眼镜",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

fn match_usb_keyword(lower: &str) -> bool {
    [
        "mouse",
        "keyboard",
        "controller",
        "gamepad",
        "鼠标",
        "键盘",
        "手柄",
        "xbox",
        "webcam",
        "logitech",
        "razer",
        "corsair",
        "keychron",
        "orochi",
        "deathadder",
        "viper",
        "gpro",
        "g pro",
        "basilisk",
        "naga",
        "blackwidow",
        "hunters",
        "kaira",
        "steelseries",
        "hyperx",
        "coolermaster",
        "roccat",
        "zte",
        "雷蛇",
        "罗技",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

pub(crate) fn is_bt_service(pnp_id_upper: &str) -> bool {
    pnp_id_upper.starts_with("BTHLEDEVICE\\{") || pnp_id_upper.starts_with("BTHENUM\\{")
}

pub(crate) fn is_generic_hid(pnp_id_upper: &str) -> bool {
    if pnp_id_upper.contains("&COL") {
        return true;
    }
    if pnp_id_upper.starts_with("USB\\") {
        return !is_wireless_24g_by_vid_pid(pnp_id_upper);
    }
    if pnp_id_upper.starts_with("BTHLEDEVICE\\{") || pnp_id_upper.starts_with("BTHENUM\\{") {
        return true;
    }
    false
}

pub(crate) fn is_system_device(pnp_id_upper: &str) -> bool {
    pnp_id_upper.starts_with("BTH\\MS_")
}

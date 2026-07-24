use crate::device::DevType;
use crate::config;
use crate::device_data;

pub(crate) fn classify_device(name: &str, pnp_class: &str, pnp_id: &str, caption: &str) -> DevType {
    let lower_combined = format!("{} {}", name, caption).to_lowercase();

    // Check for 2.4G wireless devices by VID/PID, route by device type
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
        if is_audio(&lower_combined) { return DevType::Audio; }
        if is_usb(&lower_combined, caption) { return DevType::Usb; }
        return DevType::Other;
    }
    if pnp_class.eq_ignore_ascii_case("HIDClass") {
        if is_audio(&lower_combined) { return DevType::Audio; }
        if is_usb(&lower_combined, caption) { return DevType::Usb; }
        return DevType::Other;
    }
    if pnp_id.starts_with("USB\\") && is_usb(&lower_combined, caption) {
        return DevType::Usb;
    }
    DevType::Other
}

pub(crate) fn classify_bluetooth(name: &str) -> Option<DevType> {
    // MAC-address-only BLE devices (e.g. "Bluetooth e0:cc:f8:7f:d9:eb")
    if name.starts_with("Bluetooth ") && name.len() == 27 && name.as_bytes()[12] == b':' {
        if config::with_config(|c| c.show_unnamed_bt) {
            return Some(DevType::Other);
        }
        return None;
    }
    let lower = name.to_lowercase();
    if is_audio(&lower) { return Some(DevType::Audio); }
    if is_usb(&lower, "") { return Some(DevType::Usb); }
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
        "headphone", "headset", "earphone", "earbuds", "speaker", "耳机", "音箱", "扬声器",
        "音响", "airpods", "hifi", "dac", "amp", "glasses", "眼镜",
    ]
    .iter()
    .any(|k| lower.contains(k))
}

fn is_usb(lower_name: &str, caption: &str) -> bool {
    let combined = if caption.is_empty() {
        lower_name.to_string()
    } else {
        let mut s = String::with_capacity(lower_name.len() + 1 + caption.len());
        s.push_str(lower_name);
        s.push(' ');
        s.push_str(&caption.to_lowercase());
        s
    };
    [
        "mouse", "keyboard", "controller", "gamepad", "鼠标", "键盘", "手柄", "xbox", "webcam",
        "logitech", "razer", "corsair", "keychron", "orochi", "deathadder", "viper",
        "gpro", "g pro", "basilisk", "naga", "blackwidow", "hunters", "kaira",
        "steelseries", "hyperx", "coolermaster", "roccat", "zte", "雷蛇", "罗技",
    ]
    .iter()
    .any(|k| combined.contains(k))
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

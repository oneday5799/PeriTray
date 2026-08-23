use std::mem;
use std::sync::Mutex;
use windows::Devices::Bluetooth::{BluetoothDevice, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceInformation;
use windows_sys::core::GUID;
use windows_sys::Win32::Devices::Bluetooth::*;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};

use crate::device;

/// 蓝牙操作全局锁，防止并发操作干扰适配器状态
static BT_LOCK: Mutex<()> = Mutex::new(());

// ---------------------------------------------------------------------------
// RAII 安全包装
// ---------------------------------------------------------------------------

struct RadioFindHandle(HBLUETOOTH_RADIO_FIND);
impl Drop for RadioFindHandle {
    fn drop(&mut self) {
        unsafe {
            BluetoothFindRadioClose(self.0);
        }
    }
}

struct DeviceFindHandle(HBLUETOOTH_DEVICE_FIND);
impl Drop for DeviceFindHandle {
    fn drop(&mut self) {
        unsafe {
            BluetoothFindDeviceClose(self.0);
        }
    }
}

struct RadioHandle(HANDLE);
impl Drop for RadioHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

// ---------------------------------------------------------------------------
// 辅助函数
// ---------------------------------------------------------------------------

/// BLUETOOTH_ADDRESS → "XXXXXXXXXXXX" 字符串（大写，无冒号，12 位十六进制）
fn bt_address_to_string(addr: &BLUETOOTH_ADDRESS) -> String {
    let bytes = unsafe { addr.Anonymous.rgBytes };
    format!(
        "{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}",
        bytes[5], bytes[4], bytes[3], bytes[2], bytes[1], bytes[0]
    )
}

/// 将 [u16; 248] UTF-16 数组转换为 Rust String
fn utf16_array_to_string(arr: &[u16; 248]) -> String {
    let len = arr.iter().position(|&c| c == 0).unwrap_or(248);
    String::from_utf16_lossy(&arr[..len])
}

/// 格式化 GUID 为字符串
fn guid_to_string(guid: &GUID) -> String {
    format!(
        "{{{:08X}-{:04X}-{:04X}-{:02X}{:02X}-{:02X}{:02X}{:02X}{:02X}{:02X}{:02X}}}",
        guid.data1,
        guid.data2,
        guid.data3,
        guid.data4[0],
        guid.data4[1],
        guid.data4[2],
        guid.data4[3],
        guid.data4[4],
        guid.data4[5],
        guid.data4[6],
        guid.data4[7]
    )
}

// ---------------------------------------------------------------------------
// 原生蓝牙连接/断开（替代 C#/PowerShell 实现）
// ---------------------------------------------------------------------------

/// 在指定 radio 上搜索目标 MAC 设备
fn find_device_on_radio(
    radio: HANDLE,
    target_mac: &str,
    log: &mut Vec<String>,
) -> Option<BLUETOOTH_DEVICE_INFO> {
    let mut search_params: BLUETOOTH_DEVICE_SEARCH_PARAMS = unsafe { mem::zeroed() };
    search_params.dwSize = mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32;
    search_params.fReturnAuthenticated = 1;
    search_params.fReturnRemembered = 1;
    search_params.fReturnConnected = 1;
    search_params.hRadio = radio;

    let mut device_info: BLUETOOTH_DEVICE_INFO = unsafe { mem::zeroed() };
    device_info.dwSize = mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;

    let h_find = unsafe { BluetoothFindFirstDevice(&search_params, &mut device_info) };
    if h_find.is_null() {
        return None;
    }
    let _find_guard = DeviceFindHandle(h_find);

    loop {
        let addr_str = bt_address_to_string(&device_info.Address);
        let name = utf16_array_to_string(&device_info.szName);
        log.push(format!("SEARCH addr={} name={}", addr_str, name));
        if addr_str == target_mac {
            return Some(device_info);
        }
        if unsafe { BluetoothFindNextDevice(h_find, &mut device_info) } == 0 {
            break;
        }
    }
    None
}

/// 枚举设备已安装的蓝牙服务
fn enumerate_device_services(
    radio: HANDLE,
    device: &BLUETOOTH_DEVICE_INFO,
    log: &mut Vec<String>,
) -> Vec<GUID> {
    let mut svc_count: u32 = 32;
    let buffer_size = (svc_count as usize) * mem::size_of::<GUID>();
    let mut buffer: Vec<u8> = vec![0u8; buffer_size];
    let p_guids = buffer.as_mut_ptr() as *mut GUID;

    let r = unsafe { BluetoothEnumerateInstalledServices(radio, device, &mut svc_count, p_guids) };
    log.push(format!("ENUM_RESULT:{} SVC_COUNT:{}", r, svc_count));

    let mut guids = Vec::new();
    if r == 0 && svc_count > 0 {
        for i in 0..svc_count as usize {
            let guid = unsafe { *p_guids.add(i) };
            log.push(format!("SVC[{}]:{}", i, guid_to_string(&guid)));
            guids.push(guid);
        }
    }
    guids
}

/// 在单个 radio 上尝试执行蓝牙操作
fn try_bt_action(radio: HANDLE, target_mac: &str, action: &str, log: &mut Vec<String>) -> bool {
    const DEFAULT_BT_GUIDS: &[GUID] = &[
        GUID {
            data1: 0x0000110b,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
        GUID {
            data1: 0x0000110c,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
        GUID {
            data1: 0x0000110e,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
        GUID {
            data1: 0x0000111e,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
        GUID {
            data1: 0x0000111f,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
        GUID {
            data1: 0x00001108,
            data2: 0x0000,
            data3: 0x1000,
            data4: [0x80, 0x00, 0x00, 0x80, 0x5f, 0x9b, 0x34, 0xfb],
        },
    ];

    let device = match find_device_on_radio(radio, target_mac, log) {
        Some(d) => d,
        None => {
            log.push("NOT_ON_RADIO".into());
            return false;
        }
    };

    let name = utf16_array_to_string(&device.szName);
    log.push(format!("FOUND:{} connected={}", name, device.fConnected));

    let real_guids = enumerate_device_services(radio, &device, log);

    if real_guids.is_empty() {
        log.push("USING_DEFAULT_SVCS".into());
    }
    let guids: &[GUID] = if real_guids.is_empty() {
        DEFAULT_BT_GUIDS
    } else {
        &real_guids
    };

    const MAX_RETRY: u32 = 3;

    if action == "disconnect" {
        let mut disabled = 0u32;
        for svc in guids {
            let mut ok = false;
            for retry in 0..MAX_RETRY {
                let r = unsafe {
                    BluetoothSetServiceState(radio, &device, svc, BLUETOOTH_SERVICE_DISABLE)
                };
                log.push(format!(
                    "DIS:{} -> {} (attempt {})",
                    guid_to_string(svc),
                    r,
                    retry + 1
                ));
                if r == 0 {
                    ok = true;
                    break;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
            if ok {
                disabled += 1;
            } else {
                log.push(format!(
                    "DIS_FAILED:{} after {} attempts",
                    guid_to_string(svc),
                    MAX_RETRY
                ));
            }
        }
        log.push(format!("DISABLED:{}/{}", disabled, guids.len()));
    } else if action == "connect" {
        let mut disabled = 0u32;
        for svc in guids {
            let r =
                unsafe { BluetoothSetServiceState(radio, &device, svc, BLUETOOTH_SERVICE_DISABLE) };
            if r == 0 {
                disabled += 1;
            }
        }
        log.push(format!("PRE_DISABLE:{}/{}", disabled, guids.len()));

        std::thread::sleep(std::time::Duration::from_millis(1000));

        let mut enabled = 0u32;
        for svc in guids {
            let r =
                unsafe { BluetoothSetServiceState(radio, &device, svc, BLUETOOTH_SERVICE_ENABLE) };
            log.push(format!("EN:{} -> {}", guid_to_string(svc), r));
            if r == 0 {
                enabled += 1;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        log.push(format!("ENABLED:{}/{}", enabled, guids.len()));
    }

    true
}

/// 原生蓝牙连接/断开操作（直接调用 Win32 BluetoothApis.dll）
fn bt_action_native(name: &str, action: &str) -> Result<String, String> {
    let mut log: Vec<String> = Vec::new();
    log.push(format!("START action={} name={}", action, name));

    let device_id = match device::get_device_id_by_name(name) {
        Some(id) => id,
        None => {
            log.push("DEVICE_NOT_FOUND".into());
            return Err(log.join("\n"));
        }
    };
    let mac = device_id
        .rsplit('-')
        .next()
        .unwrap_or("")
        .to_uppercase()
        .replace(':', "");
    log.push(format!("MAC:{} device_id={}", mac, device_id));

    let mut r_params: BLUETOOTH_FIND_RADIO_PARAMS = unsafe { mem::zeroed() };
    r_params.dwSize = mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32;
    let mut h_radio: HANDLE = std::ptr::null_mut();

    let h_radio_find = unsafe { BluetoothFindFirstRadio(&r_params, &mut h_radio) };
    if h_radio_find.is_null() {
        let err = unsafe { windows_sys::Win32::Foundation::GetLastError() };
        log.push(format!("NO_RADIO:win32_error={}", err));
        return Err(log.join("\n"));
    }
    let _radio_find_guard = RadioFindHandle(h_radio_find);
    log.push(format!("RADIO_OK handle={:?}", h_radio));

    let mut device_found;

    {
        let _guard = RadioHandle(h_radio);
        device_found = try_bt_action(h_radio, &mac, action, &mut log);
    }

    if !device_found {
        log.push("TRY_NEXT_RADIOS".into());
        let mut next_radio: HANDLE = std::ptr::null_mut();
        while unsafe { BluetoothFindNextRadio(h_radio_find, &mut next_radio) } != 0 {
            log.push(format!("RADIO_NEXT handle={:?}", next_radio));
            let _guard = RadioHandle(next_radio);
            device_found = try_bt_action(next_radio, &mac, action, &mut log);
            if device_found {
                break;
            }
        }
    }

    if !device_found {
        log.push("NOT_FOUND".into());
        return Err(log.join("\n"));
    }

    log.push("DONE".into());
    Ok(log.join("\n"))
}

/// 执行蓝牙连接/断开操作
pub fn bt_action(name: &str, action: &str) -> Result<String, String> {
    let _guard = crate::state::lock_unpoisoned(&BT_LOCK);

    let header = format!("[bt] {} device='{}'", action.to_uppercase(), name);
    crate::process::append_log(&header);

    let result = bt_action_native(name, action)?;

    crate::process::append_log(&format!("[bt] result: {}", result));

    Ok(result)
}

// ---------------------------------------------------------------------------
// WinRT 蓝牙设备枚举（原有代码，保持不变）
// ---------------------------------------------------------------------------

type BtDeviceInfo = (String, bool, String);

fn classic_device_from_info(device_info: &DeviceInformation) -> Option<BtDeviceInfo> {
    use windows::Devices::Bluetooth::BluetoothConnectionStatus;
    let device_id = device_info.Id().ok()?;
    let device = BluetoothDevice::FromIdAsync(&device_id).ok()?.join().ok()?;
    let name = device.Name().ok()?.to_string();
    let connected = device.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected;
    Some((name, connected, device_id.to_string()))
}

fn ble_device_from_info(device_info: &DeviceInformation) -> Option<BtDeviceInfo> {
    use windows::Devices::Bluetooth::BluetoothConnectionStatus;
    let device_id = device_info.Id().ok()?;
    let device = BluetoothLEDevice::FromIdAsync(&device_id)
        .ok()?
        .join()
        .ok()?;
    let name = device.Name().ok()?.to_string();
    let connected = device.ConnectionStatus().ok()? == BluetoothConnectionStatus::Connected;
    Some((name, connected, device_id.to_string()))
}

pub fn find_paired_bluetooth_devices(
) -> Result<Vec<(String, bool, Option<u8>, String)>, Box<dyn std::error::Error>> {
    let mut result = Vec::new();

    let btc_selector = BluetoothDevice::GetDeviceSelectorFromPairingState(true)?;
    let btc_devices_info = DeviceInformation::FindAllAsyncAqsFilter(&btc_selector)?.join()?;
    for device_info in btc_devices_info.into_iter() {
        if let Some((name, connected, device_id)) = classic_device_from_info(&device_info) {
            let battery = read_btc_battery_from_device_id(&device_id);
            result.push((name, connected, battery, device_id));
        }
    }

    let ble_selector = BluetoothLEDevice::GetDeviceSelectorFromPairingState(true)?;
    let ble_devices_info = DeviceInformation::FindAllAsyncAqsFilter(&ble_selector)?.join()?;
    for device_info in ble_devices_info.into_iter() {
        if let Some((name, connected, device_id)) = ble_device_from_info(&device_info) {
            let battery = read_ble_battery_from_id(&device_id);
            result.push((name, connected, battery, device_id));
        }
    }

    crate::process::append_log(&format!(
        "[bt] find_paired_bluetooth_devices: found {} devices",
        result.len()
    ));
    Ok(result)
}

fn read_ble_battery_from_id(device_id: &str) -> Option<u8> {
    let hstr = windows::core::HSTRING::from(device_id);
    let future = BluetoothLEDevice::FromIdAsync(&hstr).ok()?;
    let device = future.join().ok()?;
    read_ble_battery(&device)
}

fn read_ble_battery(ble_device: &BluetoothLEDevice) -> Option<u8> {
    use windows::Devices::Bluetooth::GenericAttributeProfile::{
        GattCharacteristicUuids, GattServiceUuids,
    };
    use windows::Storage::Streams::DataReader;

    let battery_service = GattServiceUuids::Battery().ok()?;
    let battery_level = GattCharacteristicUuids::BatteryLevel().ok()?;

    let services = ble_device
        .GetGattServicesForUuidAsync(battery_service)
        .ok()?
        .join()
        .ok()?;
    let service = services.Services().ok()?.into_iter().next()?;

    let chars = service
        .GetCharacteristicsForUuidAsync(battery_level)
        .ok()?
        .join()
        .ok()?;
    let char = chars.Characteristics().ok()?.into_iter().next()?;

    let buffer = char.ReadValueAsync().ok()?.join().ok()?.Value().ok()?;
    let reader = DataReader::FromBuffer(&buffer).ok()?;
    let level = reader.ReadByte().ok()?;
    Some(level)
}

/// Check connection status of a single Bluetooth device by name
pub fn check_device_connection(name: &str) -> Option<bool> {
    let cn = crate::dedup::core_name(name);
    find_paired_bluetooth_devices()
        .ok()?
        .into_iter()
        .find(|(n, _, _, _)| crate::dedup::core_name(n) == cn)
        .map(|(_, connected, _, _)| connected)
}

fn read_btc_battery_from_device_id(device_id: &str) -> Option<u8> {
    let mac = device_id.rsplit('-').next()?;
    let mac_upper = mac.to_uppercase().replace(':', "");

    let class_guid = windows_sys::Win32::Devices::DeviceAndDriverInstallation::GUID_DEVCLASS_SYSTEM;
    let filter = windows_pnp::PnpFilter::Contains(&["BTHENUM\\".to_string(), mac_upper.clone()]);
    let devices =
        windows_pnp::PnpEnumerator::enumerate_present_devices_and_filter_by_device_setup_class(
            class_guid, filter,
        )
        .ok()?;

    let battery_key = windows_pnp::PnpDevicePropertyKey {
        fmtid: windows_pnp_uuid::Uuid::from_u128(0x104EA319_6EE2_4701_BD47_8DDBF425BBE5),
        pid: 2,
    };

    for device in devices {
        let instance_id = &device.device_instance_id;
        if !instance_id.contains("BTHENUM\\") || !instance_id.to_uppercase().contains(&mac_upper) {
            continue;
        }

        if let Some(props) = &device.device_instance_properties {
            if let Some(windows_pnp::PnpDevicePropertyValue::Byte(battery)) =
                props.get(&battery_key)
            {
                return Some(*battery);
            }
        }
    }
    None
}

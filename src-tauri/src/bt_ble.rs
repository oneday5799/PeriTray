// ── BLE 连接/断开（WinRT 路径）──
//
// 参考 32feet 的 RemoteGattServer.windows.cs 和 BluetoothLEExplorer 的简单模式。
// Windows 无显式 BLE 断开 API，通过 dispose 所有 WinRT 对象释放系统级连接。

use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCommunicationStatus, GattServiceUuids, GattSession,
};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceAccessStatus;

// ── 缓存：当前唯一连接的 BLE 设备 ──

struct BLEConnection {
    device_id: String,
    device: BluetoothLEDevice,
    session: Option<GattSession>,
}

static BLE_CONN: OnceLock<Mutex<Option<BLEConnection>>> = OnceLock::new();

fn ble_conn() -> &'static Mutex<Option<BLEConnection>> {
    BLE_CONN.get_or_init(|| Mutex::new(None))
}

// ── 公开入口 ──

pub fn ble_action(device_id: &str, action: &str) -> Result<String, String> {
    match action.to_uppercase().as_str() {
        "CONNECT" => ble_connect(device_id),
        "DISCONNECT" => ble_disconnect(device_id),
        _ => Err(format!("unknown BLE action: {}", action)),
    }
}

// ── 连接 ──

fn ble_connect(device_id: &str) -> Result<String, String> {
    // 若已有连接且是同一设备，直接返回
    {
        let guard = ble_conn().lock().map_err(|e| e.to_string())?;
        if let Some(ref conn) = *guard {
            if conn.device_id == device_id {
                return Ok("already connected".into());
            }
        }
    }

    crate::process::append_verbose_log(&format!("[bt:dbg] ble_connect: device_id={}", device_id));

    // 1. 打开 BLE 设备
    let hstr = HSTRING::from(device_id);
    let device = BluetoothLEDevice::FromIdAsync(&hstr)
        .map_err(|e| format!("FromIdAsync error: {}", e))?
        .join()
        .map_err(|e| format!("FromIdAsync join error: {}", e))?;
    crate::process::append_verbose_log("[bt:dbg] ble_connect: BluetoothLEDevice opened");

    // 1.5 检查访问权限（失败时需 close device）
    let access_status = match device.RequestAccessAsync() {
        Ok(op) => op
            .join()
            .map_err(|e| format!("RequestAccessAsync join error: {}", e))?,
        Err(e) => {
            let _ = device.Close();
            return Err(format!("RequestAccessAsync error: {}", e));
        }
    };
    if access_status != DeviceAccessStatus::Allowed {
        let _ = device.Close();
        return Err(format!("access denied: {:?}", access_status));
    }
    crate::process::append_verbose_log("[bt:dbg] ble_connect: access allowed");

    // 2. 创建 GattSession（可能失败，fallback 到简单模式）
    let session = match create_session(&device) {
        Ok(s) => {
            crate::process::append_verbose_log("[bt:dbg] ble_connect: GattSession created");
            Some(s)
        }
        Err(e) => {
            crate::process::append_verbose_log(&format!(
                "[bt:dbg] ble_connect: GattSession failed ({})，使用简单模式",
                e
            ));
            None
        }
    };

    // 3. MaintainConnection = true（失败时需 close session + device）
    if let Some(ref s) = session {
        if s.CanMaintainConnection().unwrap_or(false) {
            if let Err(e) = s.SetMaintainConnection(true) {
                let _ = session.as_ref().map(|s| s.Close());
                let _ = device.Close();
                return Err(format!("SetMaintainConnection error: {}", e));
            }
            crate::process::append_verbose_log("[bt:dbg] ble_connect: MaintainConnection=true");
        }
    }

    // 4. GATT 请求触发实际连接（重试 3 次）
    let mut gatt_ok = false;
    for attempt in 0..3 {
        match trigger_connection(&device) {
            Ok(()) => {
                gatt_ok = true;
                break;
            }
            Err(e) => {
                crate::process::append_verbose_log(&format!(
                    "[bt:dbg] ble_connect: GATT attempt {} failed: {}",
                    attempt + 1,
                    e
                ));
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
    if !gatt_ok {
        crate::process::append_verbose_log(
            "[bt:dbg] ble_connect: GATT request not confirmed, caching anyway",
        );
    }

    // 5. 缓存连接
    let conn = BLEConnection {
        device_id: device_id.to_string(),
        device,
        session,
    };
    *ble_conn().lock().map_err(|e| e.to_string())? = Some(conn);

    crate::process::append_verbose_log("[bt:dbg] ble_connect: done");
    Ok("connected".into())
}

// ── 断开 ──

fn ble_disconnect(device_id: &str) -> Result<String, String> {
    let mut guard = ble_conn().lock().map_err(|e| e.to_string())?;

    if let Some(conn) = guard.take() {
        if conn.device_id == device_id {
            crate::process::append_verbose_log(&format!(
                "[bt:dbg] ble_disconnect: closing connection for {}",
                device_id
            ));
            // 显式 Close() 释放 WinRT BLE 连接资源，再 drop 释放 Rust 所有权
            let _ = conn.session.as_ref().map(|s| s.Close());
            let _ = conn.device.Close();
            drop(conn.session);
            drop(conn.device);
            crate::process::append_verbose_log("[bt:dbg] ble_disconnect: done");
            Ok("disconnected".into())
        } else {
            let cached_id = conn.device_id.clone();
            *guard = Some(conn);
            Err(format!(
                "device not connected by this app (cached: {})",
                cached_id
            ))
        }
    } else {
        Err("no BLE connection cached".into())
    }
}

// ── 辅助函数 ──

fn create_session(device: &BluetoothLEDevice) -> Result<GattSession, windows::core::Error> {
    let bt_device_id = device.BluetoothDeviceId()?;
    GattSession::FromDeviceIdAsync(&bt_device_id)?
        .join()
        .map_err(|e| windows::core::Error::from(e))
}

fn trigger_connection(device: &BluetoothLEDevice) -> Result<(), String> {
    let generic_access_uuid = GattServiceUuids::GenericAccess()
        .map_err(|e| format!("GenericAccess UUID error: {}", e))?;

    let op = device
        .GetGattServicesForUuidWithCacheModeAsync(generic_access_uuid, BluetoothCacheMode::Uncached)
        .map_err(|e| format!("GetGattServicesForUuid error: {}", e))?;

    let result = op.join().map_err(|e| format!("GATT join error: {}", e))?;

    if result.Status() == Ok(GattCommunicationStatus::Success) {
        drop(result);
        Ok(())
    } else {
        Err(format!("GATT status: {:?}", result.Status()))
    }
}

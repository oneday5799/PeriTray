// ── BLE 连接/断开（WinRT 路径）──
//
// 参考 32feet 的 RemoteGattServer.windows.cs 和 BluetoothLEExplorer 的简单模式。
// Windows 无显式 BLE 断开 API，通过 dispose 所有 WinRT 对象释放系统级连接。

use crate::{standard_log, verbose_log};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use windows::core::HSTRING;
use windows::Devices::Bluetooth::GenericAttributeProfile::{
    GattCommunicationStatus, GattServiceUuids, GattSession,
};
use windows::Devices::Bluetooth::{BluetoothCacheMode, BluetoothLEDevice};
use windows::Devices::Enumeration::DeviceAccessStatus;

// ── 缓存：当前已连接的 BLE 设备（支持多设备并行连接）──

struct BLEConnection {
    device: BluetoothLEDevice,
    session: Option<GattSession>,
}

static BLE_CONN: OnceLock<Mutex<HashMap<String, BLEConnection>>> = OnceLock::new();

/// 取 BLE 连接表的加锁入口。
///
/// 一律经 `state::lock_unpoisoned`（P2-11 的统一入口）：`BLE_CONN` 是**纯缓存**
/// （临界区内只有 `contains_key` / `remove` / `insert`），没有可被 panic 破坏的
/// 不变式，故中毒时接管内部数据继续用是安全的——这正是项目对静态量的既定策略
/// （「panic 后仅需可用，而非严格一致」）。
///
/// **为什么不用 `.lock().map_err(..)?` 把中毒上报给调用方**（本文件原先如此，
/// P3-10 收敛掉了这个「唯一例外」）：
/// ① Mutex 中毒是**永久性**的，上报的实际效果是「一次 panic 之后，蓝牙连接与断开
///    在本进程余下生命周期内彻底失效」（两个入口都在第一个加锁点就 Err），
///    而用户只会看到一句无意义的 poisoned lock；
/// ② `ble_connect` 的「换表」步骤若在 GATT 已连通后于加锁处 Err，函数会在**未改动
///    缓存**的情况下返回 ⇒ 缓存仍持有那个已被释放的旧连接，`contains_key` 为真
///    ⇒ 重试直接返回 "already connected"，而实际什么都没连上。这正是本文件在
///    GATT 未确认时特意避免的「幽灵连接」，不该在加锁失败这条路上重新引入。
///
/// 附带事实：release 是 `panic = "abort"`，中毒只可能在 debug 出现——但 debug 下
/// 的表现恰是最差的那种（永久哑掉 + 状态不一致）。
fn ble_conn() -> &'static Mutex<HashMap<String, BLEConnection>> {
    BLE_CONN.get_or_init(|| Mutex::new(HashMap::new()))
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
    standard_log!("[bt] BLE connect: {}", device_id);

    // 若已有连接且是同一设备，直接返回
    {
        let guard = crate::state::lock_unpoisoned(ble_conn());
        if guard.contains_key(device_id) {
            return Ok("already connected".into());
        }
    }

    verbose_log!("[bt:dbg] ble_connect: device_id={}", device_id);

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
            verbose_log!(
                "[bt:dbg] ble_connect: GattSession failed ({})，使用简单模式",
                e
            );
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
                verbose_log!(
                    "[bt:dbg] ble_connect: GATT attempt {} failed: {}",
                    attempt + 1,
                    e
                );
                if attempt < 2 {
                    std::thread::sleep(Duration::from_millis(200));
                }
            }
        }
    }
    if !gatt_ok {
        crate::process::append_verbose_log(
            "[bt:dbg] ble_connect: GATT request not confirmed, not caching",
        );
        // 释放资源，不缓存幽灵连接
        let _ = session.as_ref().map(|s| s.Close());
        let _ = device.Close();
        return Err("GATT request failed after 3 attempts".into());
    }

    // 5. 缓存连接（覆盖旧连接并释放其 WinRT 资源）
    let conn = BLEConnection { device, session };
    // 锁内只做「换表」这一件事：Close 是 WinRT 调用、日志是文件 I/O，都不能持锁做
    // （P3-10 锁序登记表 §四）。旧连接在锁内**摘出**（`remove` 交出所有权），
    // 锁外再释放——与 P2-7 的低电量通知同款：锁内取数据，锁外做 I/O。
    let old = {
        let mut guard = crate::state::lock_unpoisoned(ble_conn());
        let old = guard.remove(device_id);
        guard.insert(device_id.to_string(), conn);
        old
    }; // ← BLE_CONN 在此释放

    if let Some(old) = old {
        let _ = old.session.as_ref().map(|s| s.Close());
        let _ = old.device.Close();
    }

    crate::process::append_verbose_log("[bt:dbg] ble_connect: done");
    crate::process::append_log("[bt] BLE connect 完成");
    Ok("connected".into())
}

// ── 断开 ──

fn ble_disconnect(device_id: &str) -> Result<String, String> {
    standard_log!("[bt] BLE disconnect: {}", device_id);
    // 锁内只做「摘表」：Close 是 WinRT 调用、日志是文件 I/O，都不能持锁做
    // （P3-10 锁序登记表 §四）。
    let conn = {
        let mut guard = crate::state::lock_unpoisoned(ble_conn());
        guard.remove(device_id)
    }; // ← BLE_CONN 在此释放

    match conn {
        Some(conn) => {
            verbose_log!(
                "[bt:dbg] ble_disconnect: closing connection for {}",
                device_id
            );
            // 显式 Close() 释放 WinRT BLE 连接资源，再 drop 释放 Rust 所有权
            let _ = conn.session.as_ref().map(|s| s.Close());
            let _ = conn.device.Close();
            drop(conn.session);
            drop(conn.device);
            crate::process::append_verbose_log("[bt:dbg] ble_disconnect: done");
            crate::process::append_log("[bt] BLE disconnect 完成");
            Ok("disconnected".into())
        }
        None => Err("no BLE connection cached".into()),
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 钉住 `bt_ble` 所依赖的契约：`BLE_CONN` 是**无不变式的纯缓存**，
    /// 因此中毒后仍可被 `lock_unpoisoned` 接管继续使用——这正是本轮取消
    /// 「中毒上报」这个唯一例外时所依据的前提。
    ///
    /// ⚠️ **本用例的可证伪性边界（必须说清，否则就是假验收）**：
    /// 它调用的是 `lock_unpoisoned` 自身，**看不见 `bt_ble` 的调用点**——
    /// 若有人把 `ble_connect` / `ble_disconnect` 的加锁改回
    /// `.lock().map_err(..)?`，本用例**仍然通过**。
    /// 故本批的验收是**两半合起来**才完整：
    /// ① 行为侧（本用例）：helper 能接管中毒的 `BLE_CONN`；
    /// ② 结构侧（机械判据）：全仓 `.lock()` 只允许出现在 `state.rs` ——
    ///    保证所有调用点确实走了 ① 覆盖的那条路。
    /// 任一半单独都不足以支撑「蓝牙操作容忍中毒」这个结论。
    ///
    /// 注：本用例会把全局 `BLE_CONN` 永久置为中毒态（进程内无法复原），
    /// 这对其余用例无影响——没有任何用例使用 `BLE_CONN`。
    #[test]
    fn ble_conn_lock_tolerates_poison() {
        // 注入：持 BLE_CONN 时 panic，令其中毒
        // （会向 stderr 打印一行默认 hook 的 `thread panicked at ...`，属预期噪声）
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = crate::state::lock_unpoisoned(ble_conn());
            panic!("注入：持锁 panic，令 BLE_CONN 中毒");
        }));

        // 前提断言用 `try_lock` 而非 `.lock()`：后者会让本文件出现裸加锁，
        // 与「`state.rs` 之外不得出现 `.lock()`」这条机械判据冲突。
        assert!(
            matches!(
                ble_conn().try_lock(),
                Err(std::sync::TryLockError::Poisoned(_))
            ),
            "前提：注入后 BLE_CONN 必须已中毒"
        );

        // 读侧：`ble_connect` 开头的「是否已连接」判断走这条
        assert!(
            crate::state::lock_unpoisoned(ble_conn()).is_empty(),
            "中毒不应影响缓存内容"
        );
        // 摘表侧：`ble_disconnect` 与 `ble_connect` 的换表步骤走这条
        assert!(
            crate::state::lock_unpoisoned(ble_conn())
                .remove("probe")
                .is_none(),
            "中毒后仍应能正常摘表"
        );
    }
}

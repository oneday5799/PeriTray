//! 蓝牙音频设备 IKsControl 快速路径。
//!
//! 通过 Windows Core Audio API + 设备拓扑遍历，找到蓝牙音频端点的 IKsControl 接口，
//! 直接发送 KSPROPSETID_BtAudio 属性请求（RECONNECT / DISCONNECT），
//! 实现 <100ms 级别的连接/断开（对比 BluetoothSetServiceState 的 1-3s）。
//!
//! 参考：ToothTray BluetoothAudioDevices.cpp

use windows::core::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::*;
use windows::Win32::System::Com::*;
use windows::Win32::System::Variant::VARENUM;

// ── KSPROPSETID_BtAudio 及属性常量 ──

// KSPROPSETID_BtAudio 来自 Windows SDK ksmedia.h（通过 devicetopology.h 间接引入）。
// 与 windows crate 常量 KSPROPSETID_BtAudio 完全一致：{7FA06C40-B8F6-4C7E-8556-E8C33A12E54D}。
// 属性 ID 来自同文件的 KSPROPERTY_BTAUDIO 枚举：RECONNECT=0, DISCONNECT=1。
const KSPROPSETID_BT_AUDIO: GUID = GUID::from_u128(0x7fa06c40_b8f6_4c7e_8556_e8c33a12e54d);
const KSPROPERTY_ONESHOT_RECONNECT: u32 = 0;
const KSPROPERTY_ONESHOT_DISCONNECT: u32 = 1;

// ── PKEY_Device_FriendlyName（windows crate 未导出此 PID）──

const PKEY_DEVICE_FRIENDLY_NAME: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
    pid: 14,
};

// ── 蓝牙音频驱动设备 ID 前缀（bthenum / bthhfenum）──

const BT_AUDIO_ID_PREFIX: &str = r"{2}.\\?\bth";

// ── 公共接口 ──

/// 对蓝牙音频设备执行 IKsControl 快速路径连接/断开。
/// 成功返回 Ok(日志)，失败返回 Err(原因) 供调用方降级到 bt_action_native。
///
/// 注意：部分硬件的 bthenum 驱动可能不支持 KSPROPSETID_BtAudio（返回 0x80070492），
/// 此时快速路径会失败并降级到 bt_action_native，不影响功能正确性。
pub fn bt_action_ks(name: &str, action: &str) -> std::result::Result<String, String> {
    let mut log_lines: Vec<String> = Vec::new();
    log_lines.push(format!("[ks] {} device='{}'", action.to_uppercase(), name));

    // spawn_blocking 线程可能未初始化 COM，需显式初始化为 STA（与 ToothTray UI 线程一致）
    unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            log_lines.push(format!("[ks] CoInitializeEx failed: 0x{:08X}", hr.0));
        }
    }

    let controls = find_bluetooth_ks_controls(name, &mut log_lines).map_err(|e| {
        log_lines.push(format!("[ks] 查找 IKsControl 失败: {}", e));
        log_lines.join("\n")
    })?;

    if controls.is_empty() {
        log_lines.push("[ks] 未找到匹配的蓝牙音频端点".into());
        return Err(log_lines.join("\n"));
    }

    log_lines.push(format!("[ks] 找到 {} 个 IKsControl", controls.len()));

    let property_id = match action {
        "connect" => KSPROPERTY_ONESHOT_RECONNECT,
        "disconnect" => KSPROPERTY_ONESHOT_DISCONNECT,
        _ => return Err(format!("[ks] 未知操作: {}", action)),
    };

    let ks_property = KSIDENTIFIER {
        Anonymous: KSIDENTIFIER_0 {
            Anonymous: KSIDENTIFIER_0_0 {
                Set: KSPROPSETID_BT_AUDIO,
                Id: property_id,
                Flags: KSPROPERTY_TYPE_GET,
            },
        },
    };

    let mut success_count = 0u32;
    let mut fail_count = 0u32;

    for (i, ks_control) in controls.iter().enumerate() {
        let mut bytes_returned = 0u32;
        let result = unsafe {
            ks_control.KsProperty(
                &ks_property,
                std::mem::size_of::<KSIDENTIFIER>() as u32,
                std::ptr::null_mut(),
                0,
                &mut bytes_returned,
            )
        };
        match result {
            Ok(()) => {
                log_lines.push(format!("[ks] IKsControl[{}] {} 成功", i, action));
                success_count += 1;
            }
            Err(e) => {
                log_lines.push(format!("[ks] IKsControl[{}] {} 失败: {}", i, action, e));
                fail_count += 1;
            }
        }
    }

    log_lines.push(format!(
        "[ks] 完成: {} 成功, {} 失败",
        success_count, fail_count
    ));

    if success_count > 0 {
        Ok(log_lines.join("\n"))
    } else {
        Err(log_lines.join("\n"))
    }
}

// ── 内部实现 ──

/// 在所有渲染音频端点中查找与目标设备名匹配的蓝牙音频 IKsControl。
fn find_bluetooth_ks_controls(
    target_name: &str,
    log: &mut Vec<String>,
) -> windows::core::Result<Vec<IKsControl>> {
    unsafe {
        let enumerator: IMMDeviceEnumerator =
            CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
        let collection =
            enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL))?;
        let count = collection.GetCount()?;
        log.push(format!("[ks] 枚举到 {} 个渲染端点", count));

        let target_lower = target_name.to_lowercase();
        let mut controls: Vec<IKsControl> = Vec::new();

        for i in 0..count {
            let device = match collection.Item(i) {
                Ok(d) => d,
                Err(_) => continue,
            };

            let friendly_name = match read_friendly_name(&device) {
                Ok(name) => name,
                Err(_) => continue,
            };

            let ks_controls = match find_bt_audio_controls_in_topology(&device, &enumerator, log) {
                Ok(c) => c,
                Err(_) => continue,
            };

            if ks_controls.is_empty() {
                continue;
            }

            let name_lower = friendly_name.to_lowercase();
            if name_lower.contains(&target_lower) {
                log.push(format!(
                    "[ks] 匹配端点: '{}' -> {} 个 IKsControl",
                    friendly_name,
                    ks_controls.len()
                ));
                controls.extend(ks_controls);
            }
        }

        Ok(controls)
    }
}

/// 对单个音频端点遍历设备拓扑，查找蓝牙音频驱动路径上的 IKsControl。
/// 拓扑遍历对齐 ToothTray：GetConnectedTo → IPart → GetTopologyObject → GetDeviceId。
fn find_bt_audio_controls_in_topology(
    device: &IMMDevice,
    enumerator: &IMMDeviceEnumerator,
    log: &mut Vec<String>,
) -> windows::core::Result<Vec<IKsControl>> {
    unsafe {
        let topology: IDeviceTopology = device.Activate(CLSCTX_ALL, None)?;
        let connector_count = topology.GetConnectorCount()?;
        let mut controls: Vec<IKsControl> = Vec::new();

        for i in 0..connector_count {
            let connector = match topology.GetConnector(i) {
                Ok(c) => c,
                Err(_) => continue,
            };

            // ── 4 步拓扑遍历（对齐 ToothTray）──
            // 1. GetConnectedTo → 对端 connector
            let other_connector = match connector.GetConnectedTo() {
                Ok(c) => c,
                Err(_) => continue,
            };

            // 2. IPart ← 对端 connector
            let part: IPart = match other_connector.cast() {
                Ok(p) => p,
                Err(_) => continue,
            };

            // 3. GetTopologyObject → 对端设备的 topology
            let other_topology = match part.GetTopologyObject() {
                Ok(t) => t,
                Err(_) => continue,
            };

            // 4. GetDeviceId → 对端设备的打包 ID
            let connected_device_id = match other_topology.GetDeviceId() {
                Ok(id) => id,
                Err(_) => continue,
            };

            let id_str = match connected_device_id.to_string() {
                Ok(s) => s,
                Err(_) => continue,
            };

            if !id_str
                .to_lowercase()
                .starts_with(&BT_AUDIO_ID_PREFIX.to_lowercase())
            {
                continue;
            }

            log.push(format!(
                "[ks] Connector[{}] 发现蓝牙音频驱动: {}",
                i, id_str
            ));

            // 通过复用的 enumerator 获取蓝牙设备
            let wide: Vec<u16> = id_str.encode_utf16().chain(std::iter::once(0)).collect();
            let bt_device = match enumerator.GetDevice(PCWSTR(wide.as_ptr())) {
                Ok(d) => d,
                Err(e) => {
                    log.push(format!("[ks] 获取蓝牙设备失败: {}", e));
                    continue;
                }
            };

            match bt_device.Activate::<IKsControl>(CLSCTX_ALL, None) {
                Ok(ks) => {
                    controls.push(ks);
                }
                Err(e) => {
                    log.push(format!("[ks] Activate IKsControl 失败: {}", e));
                }
            }
        }

        Ok(controls)
    }
}

/// 读取 IMMDevice 的友好名。
unsafe fn read_friendly_name(device: &IMMDevice) -> windows::core::Result<String> {
    let store = device.OpenPropertyStore(STGM_READ)?;
    let propvariant = store.GetValue(&PKEY_DEVICE_FRIENDLY_NAME)?;
    if propvariant.Anonymous.Anonymous.vt == VARENUM(31) {
        // VT_LPWSTR
        let pwstr = PWSTR(propvariant.Anonymous.Anonymous.Anonymous.pwszVal.0);
        Ok(pwstr.to_string()?)
    } else {
        Err(HRESULT(0x80004005u32 as i32).into())
    }
}

// ── 验证测试 ──

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ksproperty_direct() {
        unsafe {
            // 生产代码 spawn_blocking 线程无 COM，显式初始化为 STA
            let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);

            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).unwrap();
            let collection = enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL))
                .unwrap();
            let count = collection.GetCount().unwrap();

            for i in 0..count {
                let device = collection.Item(i).unwrap();
                let topology: IDeviceTopology = match device.Activate(CLSCTX_ALL, None) {
                    Ok(t) => t,
                    Err(_) => continue,
                };
                let connector_count = match topology.GetConnectorCount() {
                    Ok(c) => c,
                    Err(_) => continue,
                };

                for j in 0..connector_count {
                    let connector = match topology.GetConnector(j) {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    let other_connector = match connector.GetConnectedTo() {
                        Ok(c) => c,
                        Err(_) => continue,
                    };
                    let part: IPart = match other_connector.cast() {
                        Ok(p) => p,
                        Err(_) => continue,
                    };
                    let other_topology = match part.GetTopologyObject() {
                        Ok(t) => t,
                        Err(_) => continue,
                    };
                    let connected_device_id = match other_topology.GetDeviceId() {
                        Ok(id) => id,
                        Err(_) => continue,
                    };
                    let id_str = match connected_device_id.to_string() {
                        Ok(s) => s,
                        Err(_) => continue,
                    };

                    if !id_str
                        .to_lowercase()
                        .starts_with(&BT_AUDIO_ID_PREFIX.to_lowercase())
                    {
                        continue;
                    }

                    let wide: Vec<u16> = id_str.encode_utf16().chain(std::iter::once(0)).collect();
                    let bt_device = match enumerator.GetDevice(PCWSTR(wide.as_ptr())) {
                        Ok(d) => d,
                        Err(_) => continue,
                    };
                    let ks: IKsControl = match bt_device.Activate(CLSCTX_ALL, None) {
                        Ok(k) => k,
                        Err(_) => continue,
                    };

                    // 测试 SDK GUID（与 ToothTray 一致）
                    let ks_property = KSIDENTIFIER {
                        Anonymous: KSIDENTIFIER_0 {
                            Anonymous: KSIDENTIFIER_0_0 {
                                Set: KSPROPSETID_BT_AUDIO,
                                Id: KSPROPERTY_ONESHOT_RECONNECT,
                                Flags: KSPROPERTY_TYPE_GET,
                            },
                        },
                    };
                    let mut bytes_returned = 0u32;
                    let result = ks.KsProperty(
                        &ks_property,
                        std::mem::size_of::<KSIDENTIFIER>() as u32,
                        std::ptr::null_mut(),
                        0,
                        &mut bytes_returned,
                    );
                    match result {
                        Ok(()) => {
                            eprintln!("STA+SDK RECONNECT S_OK bytesReturned={}", bytes_returned)
                        }
                        Err(e) => eprintln!("STA+SDK RECONNECT FAILED 0x{:08X}", e.code().0 as u32),
                    }
                    return;
                }
            }
            eprintln!("未找到蓝牙音频端点");
        }
    }

    #[test]
    fn test_enumerate_bt_audio_endpoints() {
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        let mut log = Vec::new();

        unsafe {
            let enumerator: IMMDeviceEnumerator =
                match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("CoCreateInstance 失败: {}", e);
                        return;
                    }
                };

            let collection =
                match enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE(DEVICE_STATEMASK_ALL)) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("EnumAudioEndpoints 失败: {}", e);
                        return;
                    }
                };

            let count = match collection.GetCount() {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("GetCount 失败: {}", e);
                    return;
                }
            };

            eprintln!("枚举到 {} 个渲染端点", count);

            for i in 0..count {
                let device = match collection.Item(i) {
                    Ok(d) => d,
                    Err(_) => continue,
                };

                let state = match device.GetState() {
                    Ok(s) => s,
                    Err(_) => continue,
                };

                let friendly_name = match read_friendly_name(&device) {
                    Ok(n) => n,
                    Err(_) => "<unknown>".into(),
                };

                let id = match device.GetId() {
                    Ok(id) => match id.to_string() {
                        Ok(s) => s,
                        Err(_) => "<error>".into(),
                    },
                    Err(_) => "<error>".into(),
                };

                eprintln!(
                    "端点[{}] state={} name='{}' id='{}'",
                    i, state.0, friendly_name, id
                );

                if let Ok(topology) = device.Activate::<IDeviceTopology>(CLSCTX_ALL, None) {
                    if let Ok(connector_count) = topology.GetConnectorCount() {
                        for j in 0..connector_count {
                            if let Ok(connector) = topology.GetConnector(j) {
                                let id_str = match (|| -> windows::core::Result<String> {
                                    let other = connector.GetConnectedTo()?;
                                    let part: IPart = other.cast()?;
                                    let other_topo = part.GetTopologyObject()?;
                                    let id = other_topo.GetDeviceId()?;
                                    Ok(id.to_string()?)
                                })() {
                                    Ok(s) => s,
                                    Err(e) => {
                                        eprintln!("  Connector[{}] 拓扑遍历失败: {}", j, e);
                                        continue;
                                    }
                                };
                                let is_bt = id_str
                                    .to_lowercase()
                                    .starts_with(&BT_AUDIO_ID_PREFIX.to_lowercase());
                                let tag = if is_bt { "** BT AUDIO **" } else { "" };
                                eprintln!("  Connector[{}] -> '{}' {}", j, id_str, tag);
                                log.push(format!(
                                    "端点 '{}' connector[{}] -> '{}'",
                                    friendly_name, j, id_str
                                ));
                            }
                        }
                    }
                }
            }

            eprintln!("蓝牙音频端点: {:?}", log);
        }
    }
}

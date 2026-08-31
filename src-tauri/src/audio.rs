//! 音频设备枚举、音量与会话控制（IMMDeviceEnumerator / IAudioSessionManager2）。
//! 应用级设备路由与默认设备切换见 audio_policy 模块。

use serde::Serialize;
use std::ptr;
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_VOLUME_DOWN, VK_VOLUME_MUTE,
    VK_VOLUME_UP,
};

pub use crate::audio_policy::{get_session_device, set_default_device, set_session_device};
pub(crate) use crate::audio_policy::{CLSID_POLICY_CONFIG, IID_IUNKNOWN};

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    pub volume: f32,
    pub is_muted: bool,
    pub is_default: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct VolumeChangeEvent {
    pub device_id: Option<String>,
    pub session_id: Option<String>,
    pub volume: f32,
    pub is_muted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AudioSession {
    pub id: String,
    pub name: String,
    pub icon: Arc<str>,
    pub pid: u32,
    pub volume: f32,
    pub is_muted: bool,
    pub device_id: String,
    pub is_active: bool,
}

/// 某应用已设置的输出/输入设备名（用于应用音量卡片的图标红框与悬停提示）
#[derive(Debug, Clone, Serialize)]
pub struct SessionDeviceNames {
    pub output: Option<String>,
    pub input: Option<String>,
}

/// 确保当前线程已初始化 COM（幂等调用）
pub(crate) unsafe fn ensure_com_initialized() {
    let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok();
}

/// 获取 IMMDeviceEnumerator 并执行回调
unsafe fn with_enumerator<R>(f: impl FnOnce(&IMMDeviceEnumerator) -> R) -> Result<R> {
    ensure_com_initialized();
    let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    Ok(f(&enumerator))
}

/// 枚举指定方向的音频设备（output=eRender / input=eCapture），并标记系统默认
fn enumerate_devices(flow: EDataFlow) -> Result<Vec<AudioDevice>> {
    unsafe {
        with_enumerator(|enumerator| {
            let default_id = enumerator
                .GetDefaultAudioEndpoint(flow, eMultimedia)
                .ok()
                .and_then(|d| d.GetId().ok())
                .map(|id| id.to_string().unwrap_or_default())
                .unwrap_or_default();
            let collection = enumerator.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?;
            let count = collection.GetCount()?;
            let mut devices = Vec::new();
            for i in 0..count {
                if let Ok(device) = collection.Item(i) {
                    if let Ok(id) = device.GetId() {
                        let id_str = id.to_string()?;
                        let name = get_device_name(&device)
                            .unwrap_or_else(|_| "Unknown Device".to_string());
                        let (volume, is_muted) =
                            get_device_volume_state(&device).unwrap_or((0.0, false));
                        devices.push(AudioDevice {
                            id: id_str.clone(),
                            name,
                            volume,
                            is_muted,
                            is_default: id_str == default_id,
                        });
                    }
                }
            }
            Ok(devices)
        })?
    }
}

pub fn enumerate_output_devices() -> Result<Vec<AudioDevice>> {
    enumerate_devices(eRender)
}

pub fn enumerate_input_devices() -> Result<Vec<AudioDevice>> {
    enumerate_devices(eCapture)
}
unsafe fn get_device_volume_state(device: &IMMDevice) -> Result<(f32, bool)> {
    let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
    let mute = endpoint.GetMute()?;
    Ok((endpoint.GetMasterVolumeLevelScalar()?, mute.as_bool()))
}

unsafe fn get_device_name(device: &IMMDevice) -> Result<String> {
    let store = device.OpenPropertyStore(STGM(0))?;
    let key = PROPERTYKEY {
        fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
        pid: 14,
    };
    let value = store.GetValue(&key as *const _)?;
    let name = format!("{}", value).trim().to_string();
    if name.is_empty() {
        let key_desc = PROPERTYKEY {
            fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
            pid: 2,
        };
        let value_desc = store.GetValue(&key_desc as *const _)?;
        let name_desc = format!("{}", value_desc).trim().to_string();
        if !name_desc.is_empty() {
            return Ok(name_desc);
        }
        return Ok("Unknown Audio Device".to_string());
    }
    Ok(name)
}

pub fn set_device_volume(device_id: &str, volume: f32) -> Result<()> {
    let mute_lock = crate::config::with_config(|c| c.mute_lock);
    unsafe {
        with_enumerator(|enumerator| -> Result<()> {
            let device = enumerator.GetDevice(&HSTRING::from(device_id))?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let was_muted = endpoint.GetMute()?.as_bool();
            let mut target = volume.max(0.0).min(1.0);
            if mute_lock && was_muted {
                let current = endpoint.GetMasterVolumeLevelScalar()?;
                target = target.min(current);
            }
            endpoint.SetMasterVolumeLevelScalar(target, ptr::null())?;
            Ok(())
        })??;
    }
    Ok(())
}

pub fn set_shutdown_volumes(devices: &std::collections::HashMap<String, f32>) {
    unsafe {
        let _ = with_enumerator(|enumerator| -> Result<()> {
            let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let count = collection.GetCount().unwrap_or(0);
            for i in 0..count {
                if let Ok(device) = collection.Item(i) {
                    let name = get_device_name(&device).unwrap_or_default();
                    if let Some(&level) = devices.get(&name) {
                        if let Ok(endpoint) =
                            device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                        {
                            let _ = endpoint
                                .SetMasterVolumeLevelScalar(level.max(0.0).min(1.0), ptr::null());
                            crate::process::append_log(&format!(
                                "[audio_notify] shutdown: set '{}' to {:.0}%",
                                name,
                                level * 100.0
                            ));
                        }
                    }
                }
            }
            Ok(())
        });
    }
}

pub fn toggle_device_mute(device_id: &str) -> Result<()> {
    let mut new_muted = false;
    unsafe {
        with_enumerator(|enumerator| -> Result<()> {
            let device = enumerator.GetDevice(&HSTRING::from(device_id))?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            let current = endpoint.GetMute()?;
            new_muted = !current.as_bool();
            let name = get_device_name(&device).unwrap_or_default();
            let force_mute =
                crate::config::with_config(|c| c.force_mute_devices.iter().any(|n| n == &name));
            if new_muted {
                if force_mute {
                    // 强制静音：记录静音前音量，模拟两次静音（设备在最低音量下才真正静音）
                    let pre = endpoint.GetMasterVolumeLevelScalar()?;
                    crate::state::lock_unpoisoned(force_mute_prev_volume()).insert(name, pre);
                    endpoint.SetMute(true, ptr::null())?;
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    endpoint.SetMute(true, ptr::null())?;
                } else {
                    endpoint.SetMute(true, ptr::null())?;
                }
            } else {
                endpoint.SetMute(false, ptr::null())?;
                if force_mute {
                    // 恢复静音前的音量
                    let mut guard = crate::state::lock_unpoisoned(force_mute_prev_volume());
                    if let Some(prev) = guard.remove(&name) {
                        let _ = endpoint
                            .SetMasterVolumeLevelScalar(prev.max(0.0).min(1.0), ptr::null());
                    }
                }
            }
            Ok(())
        })??;
    }
    Ok(())
}

fn force_mute_prev_volume() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    static FORCE_MUTE_PREV_VOLUME: std::sync::OnceLock<
        std::sync::Mutex<std::collections::HashMap<String, f32>>,
    > = std::sync::OnceLock::new();
    FORCE_MUTE_PREV_VOLUME.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()))
}

pub fn set_device_mute(device_id: &str, muted: bool) -> Result<()> {
    unsafe {
        with_enumerator(|enumerator| -> Result<()> {
            let device = enumerator.GetDevice(&HSTRING::from(device_id))?;
            let endpoint: IAudioEndpointVolume = device.Activate(CLSCTX_ALL, None)?;
            endpoint.SetMute(muted, ptr::null())?;
            Ok(())
        })??;
    }
    Ok(())
}

pub fn enumerate_audio_sessions(_device_id: &str) -> Result<Vec<AudioSession>> {
    unsafe {
        with_enumerator(|enumerator| -> Result<Vec<AudioSession>> {
            let mut all_sessions: Vec<AudioSession> = Vec::new();
            let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
            let device_count = collection.GetCount()?;
            for di in 0..device_count {
                if let Ok(device) = collection.Item(di) {
                    let dev_id = device
                        .GetId()
                        .map(|id| id.to_string().unwrap_or_default())
                        .unwrap_or_default();
                    let session_manager: IAudioSessionManager2 =
                        match device.Activate(CLSCTX_ALL, None) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                    let session_enumerator = match session_manager.GetSessionEnumerator() {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let count = session_enumerator.GetCount().unwrap_or(0);
                    for i in 0..count {
                        if let Ok(session_control) = session_enumerator.GetSession(i) {
                            let session_control2: IAudioSessionControl2 =
                                match session_control.cast() {
                                    Ok(s) => s,
                                    Err(_) => continue,
                                };
                            let state = session_control2.GetState().unwrap_or(AudioSessionState(0));
                            if state.0 > 2 {
                                continue;
                            }
                            let pid = session_control2.GetProcessId().unwrap_or(0);
                            if pid == 0 {
                                continue;
                            }
                            let session_id = get_session_id(&session_control2).unwrap_or_default();
                            let (volume, is_muted) =
                                if let Ok(sv) = session_control.cast::<ISimpleAudioVolume>() {
                                    let vol = sv.GetMasterVolume().unwrap_or(0.0);
                                    let muted = sv.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                                    (vol, muted)
                                } else {
                                    (0.0, false)
                                };
                            let session_name =
                                get_session_display_name(&session_control).unwrap_or_default();
                            let display_name = if session_name.starts_with('@') {
                                "System".to_string()
                            } else if !session_name.is_empty() && session_name != "Unknown App" {
                                session_name
                            } else {
                                crate::app_icon::get_process_name_by_pid(pid)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("App (PID: {})", pid))
                            };
                            let icon =
                                crate::app_icon::get_app_icon_by_pid(pid).unwrap_or_default();
                            let is_active = state.0 == 1;
                            let audio_session = AudioSession {
                                id: session_id,
                                name: display_name,
                                icon,
                                pid,
                                volume,
                                is_muted,
                                device_id: dev_id.clone(),
                                is_active,
                            };
                            if let Some(existing) = all_sessions.iter_mut().find(|s| s.pid == pid) {
                                if is_active && !existing.is_active {
                                    *existing = audio_session;
                                }
                            } else {
                                all_sessions.push(audio_session);
                            }
                        }
                    }
                }
            }
            Ok(all_sessions)
        })?
    }
}

/// 按 session_id 查找并返回 ISimpleAudioVolume 接口
unsafe fn find_session_volume(session_id: &str) -> Result<ISimpleAudioVolume> {
    with_enumerator(|enumerator| -> Result<ISimpleAudioVolume> {
        let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
        for di in 0..collection.GetCount().unwrap_or(0) {
            if let Ok(device) = collection.Item(di) {
                let sm: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) {
                    Ok(m) => m,
                    Err(_) => continue,
                };
                let se = match sm.GetSessionEnumerator() {
                    Ok(e) => e,
                    Err(_) => continue,
                };
                for i in 0..se.GetCount().unwrap_or(0) {
                    if let Ok(sc) = se.GetSession(i) {
                        let sc2: IAudioSessionControl2 = match sc.cast() {
                            Ok(s) => s,
                            Err(_) => continue,
                        };
                        if get_session_id(&sc2).unwrap_or_default() == session_id {
                            if let Ok(sv) = sc.cast::<ISimpleAudioVolume>() {
                                return Ok(sv);
                            }
                        }
                    }
                }
            }
        }
        Err(Error::empty())
    })?
}

pub fn set_session_volume(session_id: &str, volume: f32) -> Result<()> {
    unsafe {
        let sv = find_session_volume(session_id)?;
        sv.SetMasterVolume(volume.max(0.0).min(1.0), ptr::null())?;
    }
    Ok(())
}

pub fn set_session_mute(session_id: &str, muted: bool) -> Result<()> {
    unsafe {
        let sv = find_session_volume(session_id)?;
        sv.SetMute(muted, ptr::null())?;
    }
    Ok(())
}

/// 解析给定一批 pid 各自已设置的输出/输入设备名（id→名，取系统友好名）。
/// 未设置覆盖、设备不可解析（已断开等）时为 None。
pub fn resolve_session_device_names(
    pids: &[u32],
) -> std::collections::HashMap<u32, SessionDeviceNames> {
    let out_map: std::collections::HashMap<String, String> = enumerate_output_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();
    let in_map: std::collections::HashMap<String, String> = enumerate_input_devices()
        .unwrap_or_default()
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();

    let mut result = std::collections::HashMap::new();
    for &pid in pids {
        let output = get_session_device(pid, "output")
            .ok()
            .flatten()
            .and_then(|id| out_map.get(&id).cloned());
        let input = get_session_device(pid, "input")
            .ok()
            .flatten()
            .and_then(|id| in_map.get(&id).cloned());
        if output.is_some() || input.is_some() {
            result.insert(pid, SessionDeviceNames { output, input });
        }
    }
    result
}

unsafe fn get_session_display_name(session: &IAudioSessionControl) -> Result<String> {
    let display_name = session.GetDisplayName()?;
    if display_name.is_empty() {
        return Ok("Unknown App".to_string());
    }
    Ok(display_name.to_string()?)
}

unsafe fn get_session_id(session: &IAudioSessionControl2) -> Result<String> {
    let id = session.GetSessionInstanceIdentifier()?;
    Ok(id.to_string()?)
}

fn simulate_media_key(vk: u16) {
    unsafe {
        keybd_event(vk as u8, 0, KEYEVENTF_EXTENDEDKEY, 0);
        keybd_event(vk as u8, 0, KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP, 0);
    }
}

pub fn adjust_default_volume_up() {
    simulate_media_key(VK_VOLUME_UP);
}

pub fn adjust_default_volume_down() {
    simulate_media_key(VK_VOLUME_DOWN);
}

pub fn toggle_default_mute() {
    simulate_media_key(VK_VOLUME_MUTE);
}

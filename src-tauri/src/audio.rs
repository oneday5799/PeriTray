use serde::Serialize;
use std::ffi::c_void;
use std::ptr;
use std::sync::Arc;
use windows::core::*;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{keybd_event, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, VK_VOLUME_DOWN, VK_VOLUME_MUTE, VK_VOLUME_UP};

#[derive(Debug, Clone, Serialize)]
pub struct AudioDevice { pub id: String, pub name: String, pub volume: f32, pub is_muted: bool, pub is_default: bool }

#[derive(Debug, Clone, Serialize)]
pub struct VolumeChangeEvent { pub device_id: Option<String>, pub session_id: Option<String>, pub volume: f32, pub is_muted: bool }

#[derive(Debug, Clone, Serialize)]
pub struct AudioSession { pub id: String, pub name: String, pub icon: Arc<str>, pub pid: u32, pub volume: f32, pub is_muted: bool, pub device_id: String, pub is_active: bool }

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

pub fn set_default_device(device_id: &str) -> Result<()> {
    crate::process::append_log(&format!("[audio] set_default_device: {}", device_id));
    unsafe {
        ensure_com_initialized();
        let wide: Vec<u16> = crate::process::to_wide(device_id);
        set_default_device_raw(wide.as_ptr())?;
        Ok(())
    }
}

unsafe fn set_default_device_raw(wide_ptr: *const u16) -> Result<()> {
    let policy_config_cls = windows_sys::core::GUID {
        data1: 0x870af99c, data2: 0x171d, data3: 0x4f9e,
        data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
    };
    let ipolicy_iid = windows_sys::core::GUID {
        data1: 0xf8679f50, data2: 0x850a, data3: 0x41cf,
        data4: [0x9c, 0x72, 0x43, 0x0f, 0x29, 0x02, 0x90, 0xc8],
    };
    let iid_unknown = windows_sys::core::GUID {
        data1: 0x00000000, data2: 0x0000, data3: 0x0000,
        data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
    };

    let mut unknown_ptr: *mut c_void = ptr::null_mut();
    let hr = windows_sys::Win32::System::Com::CoCreateInstance(
        &policy_config_cls, ptr::null_mut(), windows_sys::Win32::System::Com::CLSCTX_ALL,
        &iid_unknown,
        &mut unknown_ptr as *mut *mut _,
    );
    if hr < 0 { return Err(Error::empty()); }

    let mut policy_ptr: *mut c_void = ptr::null_mut();
    let unknown_vtable = *(unknown_ptr as *const *const usize);
    let qi_fn: unsafe extern "system" fn(*mut c_void, *const windows_sys::core::GUID, *mut *mut c_void) -> i32 =
        std::mem::transmute(*unknown_vtable);
    let qi_hr = qi_fn(unknown_ptr, &ipolicy_iid, &mut policy_ptr);

    let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*(unknown_vtable.offset(2)));
    release_fn(unknown_ptr);

    if qi_hr < 0 || policy_ptr.is_null() { return Err(Error::empty()); }

    let policy_vtable = *(policy_ptr as *const *const usize);
    let set_endpoint_fn: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> i32 =
        std::mem::transmute(*policy_vtable.add(13));

    let mut all_ok = true;
    for role in 0..=2 {
        let hr = set_endpoint_fn(policy_ptr, PCWSTR(wide_ptr), role);
        if hr < 0 { all_ok = false; }
    }

    let release_fn2: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*(policy_vtable.offset(2)));
    release_fn2(policy_ptr);

    if !all_ok { return Err(Error::empty()); }
    Ok(())
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
                        let name = get_device_name(&device).unwrap_or_else(|_| "Unknown Device".to_string());
                        let (volume, is_muted) = get_device_volume_state(&device).unwrap_or((0.0, false));
                        devices.push(AudioDevice { id: id_str.clone(), name, volume, is_muted, is_default: id_str == default_id });
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
    let key = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 14 };
    let value = store.GetValue(&key as *const _)?;
    let name = format!("{}", value).trim().to_string();
    if name.is_empty() {
        let key_desc = PROPERTYKEY { fmtid: GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0), pid: 2 };
        let value_desc = store.GetValue(&key_desc as *const _)?;
        let name_desc = format!("{}", value_desc).trim().to_string();
        if !name_desc.is_empty() { return Ok(name_desc); }
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
                        if let Ok(endpoint) = device.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) {
                            let _ = endpoint.SetMasterVolumeLevelScalar(level.max(0.0).min(1.0), ptr::null());
                            crate::process::append_log(&format!(
                                "[audio_notify] shutdown: set '{}' to {:.0}%", name, level * 100.0
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
            let force_mute = crate::config::with_config(|c| c.force_mute_devices.iter().any(|n| n == &name));
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
                        let _ = endpoint.SetMasterVolumeLevelScalar(prev.max(0.0).min(1.0), ptr::null());
                    }
                }
            }
            Ok(())
        })??;
    }
    Ok(())
}

fn force_mute_prev_volume() -> &'static std::sync::Mutex<std::collections::HashMap<String, f32>> {
    static FORCE_MUTE_PREV_VOLUME: std::sync::OnceLock<std::sync::Mutex<std::collections::HashMap<String, f32>>> = std::sync::OnceLock::new();
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
                    let dev_id = device.GetId().map(|id| id.to_string().unwrap_or_default()).unwrap_or_default();
                    let session_manager: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) { Ok(m) => m, Err(_) => continue };
                    let session_enumerator = match session_manager.GetSessionEnumerator() { Ok(e) => e, Err(_) => continue };
                    let count = session_enumerator.GetCount().unwrap_or(0);
                    for i in 0..count {
                        if let Ok(session_control) = session_enumerator.GetSession(i) {
                            let session_control2: IAudioSessionControl2 = match session_control.cast() { Ok(s) => s, Err(_) => continue };
                            let state = session_control2.GetState().unwrap_or(AudioSessionState(0));
                            if state.0 > 2 { continue; }
                            let pid = session_control2.GetProcessId().unwrap_or(0);
                            if pid == 0 { continue; }
                            let session_id = get_session_id(&session_control2).unwrap_or_default();
                            let (volume, is_muted) = if let Ok(sv) = session_control.cast::<ISimpleAudioVolume>() {
                                let vol = sv.GetMasterVolume().unwrap_or(0.0);
                                let muted = sv.GetMute().map(|b| b.as_bool()).unwrap_or(false);
                                (vol, muted)
                            } else {
                                (0.0, false)
                            };
                            let session_name = get_session_display_name(&session_control).unwrap_or_default();
                            let display_name = if session_name.starts_with('@') {
                                "System".to_string()
                            } else if !session_name.is_empty() && session_name != "Unknown App" { session_name } else {
                                crate::app_icon::get_process_name_by_pid(pid)
                                    .map(|s| s.to_string())
                                    .unwrap_or_else(|| format!("App (PID: {})", pid))
                            };
                            let icon = crate::app_icon::get_app_icon_by_pid(pid).unwrap_or_default();
                            let is_active = state.0 == 1;
                            let audio_session = AudioSession { id: session_id, name: display_name, icon, pid, volume, is_muted, device_id: dev_id.clone(), is_active };
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
                let sm: IAudioSessionManager2 = match device.Activate(CLSCTX_ALL, None) { Ok(m) => m, Err(_) => continue };
                let se = match sm.GetSessionEnumerator() { Ok(e) => e, Err(_) => continue };
                for i in 0..se.GetCount().unwrap_or(0) {
                    if let Ok(sc) = se.GetSession(i) {
                        let sc2: IAudioSessionControl2 = match sc.cast() { Ok(s) => s, Err(_) => continue };
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

pub fn toggle_session_mute(session_id: &str) -> Result<()> {
    unsafe {
        let sv = find_session_volume(session_id)?;
        let current = sv.GetMute()?;
        sv.SetMute(!current.as_bool(), ptr::null())?;
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

#[link(name = "kernel32")]
extern "system" {
    fn LoadLibraryA(name: *const u8) -> *mut c_void;
    fn GetProcAddress(module: *mut c_void, name: *const u8) -> *mut c_void;
}

fn combase_module() -> *mut c_void {
    static COM_BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let m = *COM_BASE.get_or_init(|| unsafe { LoadLibraryA(b"combase.dll\0".as_ptr()) as usize });
    m as *mut c_void
}

unsafe fn combase_proc(name: &[u8]) -> *mut c_void {
    GetProcAddress(combase_module(), name.as_ptr())
}

type RoGetActivationFactoryFn = unsafe extern "system" fn(*const c_void, *const windows_sys::core::GUID, *mut *mut c_void) -> i32;
type WindowsGetStringRawBufferFn = unsafe extern "system" fn(*const c_void, *mut u32) -> *const u16;
type WindowsDeleteStringFn = unsafe extern "system" fn(*const c_void) -> i32;

fn policy_config_factory_iid() -> windows_sys::core::GUID {
    windows_sys::core::GUID {
        data1: 0xab3d4648, data2: 0xe242, data3: 0x459f,
        data4: [0xb0, 0x2f, 0x54, 0x1c, 0x70, 0x30, 0x63, 0x24],
    }
}

/// 通过 WinRT 激活工厂获取 IAudioPolicyConfigFactory（Win11 21H2+ 使用 AB3D4648 变体）
unsafe fn create_policy_config_factory() -> Result<*mut c_void> {
    ensure_com_initialized();
    let roget_ptr = combase_proc(b"RoGetActivationFactory\0");
    if roget_ptr.is_null() { return Err(Error::from_hresult(HRESULT(0x80004003u32 as i32))); }
    let roget: RoGetActivationFactoryFn = std::mem::transmute(roget_ptr);

    let class_name = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
    let class_raw: *const c_void = std::mem::transmute_copy::<HSTRING, *const c_void>(&class_name);
    let iid = policy_config_factory_iid();
    let mut factory: *mut c_void = ptr::null_mut();
    let hr = roget(class_raw, &iid, &mut factory);
    if hr < 0 || factory.is_null() { return Err(Error::from_hresult(HRESULT(hr))); }
    Ok(factory)
}

unsafe fn release_com_pointer(ptr: *mut c_void) {
    if ptr.is_null() { return; }
    let vtable = *(ptr as *const *const usize);
    let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 = std::mem::transmute(*vtable.add(2));
    release_fn(ptr);
}

/// 释放并读取 WinRT HSTRING 句柄的内容
unsafe fn read_hstring(handle: *const c_void) -> String {
    if handle.is_null() { return String::new(); }
    let mut len: u32 = 0;
    let buf_ptr = combase_proc(b"WindowsGetStringRawBuffer\0");
    let s = if buf_ptr.is_null() {
        String::new()
    } else {
        let raw_buffer: WindowsGetStringRawBufferFn = std::mem::transmute(buf_ptr);
        let buf = raw_buffer(handle, &mut len);
        if buf.is_null() {
            String::new()
        } else {
            String::from_utf16_lossy(std::slice::from_raw_parts(buf, len as usize))
        }
    };
    let del_ptr = combase_proc(b"WindowsDeleteString\0");
    if !del_ptr.is_null() {
        let del: WindowsDeleteStringFn = std::mem::transmute(del_ptr);
        let _ = del(handle);
    }
    s
}

fn direction_flow(direction: &str) -> EDataFlow {
    if direction == "input" { eCapture } else { eRender }
}

/// 该接口的“未找到/进程无音频”错误码（不同系统版本返回不同值）
fn policy_not_found(hr: i32) -> bool {
    hr as u32 == 0x80070057 || hr as u32 == 0x80070490
}

const MMDEVAPI_PREFIX: &str = r"\\?\SWD#MMDEVAPI#";
const DEVINTERFACE_AUDIO_RENDER: &str = "#{e6327cad-dcec-4949-ae8a-991e976a79d2}";
const DEVINTERFACE_AUDIO_CAPTURE: &str = "#{2eef81be-33fa-4800-9670-1cd474972c3f}";

/// 将 MMDevice ID 打包为策略 API 所需的设备接口路径
fn pack_device_id(device_id: &str, flow: EDataFlow) -> String {
    let suffix = if flow == eCapture { DEVINTERFACE_AUDIO_CAPTURE } else { DEVINTERFACE_AUDIO_RENDER };
    format!("{}{}{}", MMDEVAPI_PREFIX, device_id, suffix)
}

/// 将策略 API 返回的设备接口路径解包为 MMDevice ID
fn unpack_device_id(packed: &str) -> String {
    let mut s = packed.to_string();
    if s.starts_with(MMDEVAPI_PREFIX) { s = s[MMDEVAPI_PREFIX.len()..].to_string(); }
    for suf in [DEVINTERFACE_AUDIO_RENDER, DEVINTERFACE_AUDIO_CAPTURE] {
        if s.ends_with(suf) { s = s[..s.len() - suf.len()].to_string(); break; }
    }
    s
}

/// 设置某进程（应用）的音频输出/输入设备。device_id 为空表示恢复系统默认
pub fn set_session_device(pid: u32, direction: &str, device_id: &str) -> Result<()> {
    let flow = direction_flow(direction);
    unsafe {
        let factory = create_policy_config_factory()?;
        let vtable = *(factory as *const *const usize);
        // SetPersistedDefaultAudioEndpoint (vtable[25])：pid + flow + role + HSTRING 设备接口路径
        let set_fn: unsafe extern "system" fn(*mut c_void, u32, i32, i32, *const c_void) -> i32 =
            std::mem::transmute(*vtable.add(25));
        let packed = if device_id.is_empty() {
            None
        } else {
            Some(pack_device_id(device_id, flow))
        };
        let hstring = packed.as_ref().map(|p| HSTRING::from(p.as_str()));
        let raw = hstring.as_ref().map_or(ptr::null(), |h| {
            std::mem::transmute_copy::<HSTRING, *const c_void>(h)
        });
        for role in 0..=2 {
            let hr = set_fn(factory, pid, flow.0, role, raw);
            if hr < 0 && !policy_not_found(hr) {
                release_com_pointer(factory);
                return Err(Error::from_hresult(HRESULT(hr)));
            }
        }
        release_com_pointer(factory);
        Ok(())
    }
}

/// 查询某进程已持久化的音频设备覆盖设置，未设置时返回 None
pub fn get_session_device(pid: u32, direction: &str) -> Result<Option<String>> {
    let flow = direction_flow(direction);
    unsafe {
        let factory = create_policy_config_factory()?;
        let vtable = *(factory as *const *const usize);
        // GetPersistedDefaultAudioEndpoint (vtable[26])：pid + flow + role + HSTRING* out
        let get_fn: unsafe extern "system" fn(*mut c_void, u32, i32, i32, *mut *const c_void) -> i32 =
            std::mem::transmute(*vtable.add(26));
        let mut out: *const c_void = ptr::null();
        let hr = get_fn(factory, pid, flow.0, eMultimedia.0, &mut out);
        release_com_pointer(factory);
        if hr < 0 { return Ok(None); } // 无覆盖/进程无音频
        if out.is_null() { return Ok(None); }
        let id = read_hstring(out);
        if id.is_empty() { Ok(None) } else { Ok(Some(unpack_device_id(&id))) }
    }
}

unsafe fn get_session_display_name(session: &IAudioSessionControl) -> Result<String> {
    let display_name = session.GetDisplayName()?;
    if display_name.is_empty() { return Ok("Unknown App".to_string()); }
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

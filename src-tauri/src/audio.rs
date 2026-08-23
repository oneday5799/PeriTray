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
unsafe fn ensure_com_initialized() {
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
        let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
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

pub fn enumerate_output_devices() -> Result<Vec<AudioDevice>> {
    unsafe {
        with_enumerator(|enumerator| {
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()
                .and_then(|d| d.GetId().ok())
                .map(|id| id.to_string().unwrap_or_default())
                .unwrap_or_default();
            let collection = enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)?;
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
                    force_mute_prev_volume().lock().unwrap_or_else(|e| e.into_inner()).insert(name, pre);
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
                    let mut guard = force_mute_prev_volume().lock().unwrap_or_else(|e| e.into_inner());
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

pub fn enumerate_input_devices() -> Result<Vec<AudioDevice>> {
    unsafe {
        with_enumerator(|enumerator| {
            let default_id = enumerator
                .GetDefaultAudioEndpoint(eCapture, eMultimedia)
                .ok()
                .and_then(|d| d.GetId().ok())
                .map(|id| id.to_string().unwrap_or_default())
                .unwrap_or_default();
            let collection = enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE)?;
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
        let mut last_hr = 0;
        for role in 0..=2 {
            last_hr = set_fn(factory, pid, flow.0, role, raw);
            if last_hr < 0 && !policy_not_found(last_hr) {
                release_com_pointer(factory);
                return Err(Error::from_hresult(HRESULT(last_hr)));
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

// ===== 空间音效（CPolicyConfigClient 未公开扩展接口，槽位经 PDB 符号 + 运行时双重验证）=====

#[derive(Debug, Clone, Serialize)]
pub struct SpatialSoundFormat { pub guid: String, pub name: String }

#[derive(Debug, Clone, Serialize)]
pub struct SpatialSoundState { pub current: Option<String>, pub supported: Vec<SpatialSoundFormat> }

/// 已知空间音效格式表（GUID 均为本机 Win11 实测，Sonic 与 NirSoft svcl 文档一致）。
/// 第三项为格式提供应用的包族名（AppX PackageFamilyName）：
/// None = 系统内置恒可用；Some = 需对应商店应用已为当前用户注册，卸载后从菜单移除
const SPATIAL_SOUND_FORMATS: &[(&str, &str, Option<&[&str]>)] = &[
    ("b53d940c-b846-4831-9f76-d102b9b725a0", "用于耳机的 Windows Sonic", None),
    (
        "1459ac38-3875-49bf-bb59-0fe80f4d395d",
        "Dolby Atmos for Headphones",
        Some(&[
            "DolbyLaboratories.DolbyAtmosforHeadphones_rz1tebttyb220",
            "DolbyLaboratories.DolbyAccess_rz1tebttyb220",
        ]),
    ),
    (
        "4444acb0-8dc0-4c2c-a0d8-2c76db470f86",
        "DTS Headphone:X",
        Some(&["DTSInc.DTSSoundUnbound_t5j2fzbtdg37r"]),
    ),
];

/// Get/SetDeviceSpatialSettings 写读的状态块有效长度（0x48 之外为堆噪声）
const SPATIAL_STATE_LEN: usize = 0x48;

pub fn get_spatial_sound(device_id: &str) -> std::result::Result<SpatialSoundState, String> {
    let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let client = PolicySpatialClient::acquire()?;
        let (state, fmt_ptr) = client.query(wide.as_ptr())?;
        CoTaskMemFree(Some(fmt_ptr as *const c_void));
        if !validate_state_encoding(&state) {
            return Err("当前系统版本的空间音效接口布局不受支持".to_string());
        }
        Ok(SpatialSoundState {
            current: state_current_guid(&state),
            supported: SPATIAL_SOUND_FORMATS.iter()
                .filter(|(_, _, pkgs)| spatial_format_available(pkgs))
                .map(|(g, n, _)| SpatialSoundFormat { guid: g.to_string(), name: n.to_string() })
                .collect(),
        })
    }
}

/// 查询当前用户是否注册了指定包族的 AppX 应用（免管理员，经 PackageManager WinRT）
fn is_package_registered_for_user(family: &str) -> bool {
    use windows::Management::Deployment::PackageManager;
    unsafe {
        ensure_com_initialized();
        let Ok(pm) = PackageManager::new() else { return false; };
        let empty = HSTRING::from("");
        let fam = HSTRING::from(family);
        pm.FindPackagesByUserSecurityIdPackageFamilyName(&empty, &fam)
            .map(|p| p.into_iter().next().is_some())
            .unwrap_or(false)
    }
}

/// 格式可用性：无包族依赖（内置）恒可用；有依赖则任一包族已注册即视为可用
fn spatial_format_available(pkgs: &Option<&[&str]>) -> bool {
    match pkgs {
        None => true,
        Some(families) => families.iter().any(|f| is_package_registered_for_user(f)),
    }
}

pub fn set_spatial_sound(device_id: &str, format_guid: Option<&str>) -> std::result::Result<(), String> {
    let guid_hex = match format_guid.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(parse_guid_str(s).ok_or_else(|| format!("无效的格式 GUID：{}", s))?),
    };
    let entry = SPATIAL_SOUND_FORMATS.iter().find(|(g, _, _)| parse_guid_str(g) == guid_hex);
    // 已知格式需校验提供应用是否仍安装，防止写入已卸载格式
    if let (Some(_), Some((_, name, pkgs))) = (&guid_hex, entry) {
        if !spatial_format_available(pkgs) {
            return Err(format!("{} 未安装或已被卸载", name));
        }
    }
    let target_name = match (&guid_hex, entry) {
        (_, Some((_, n, _))) => *n,
        (None, _) => "关",
        _ => "自定义格式",
    };
    crate::process::append_log(&format!("[audio] set_spatial_sound: {} -> {}", device_id, target_name));
    let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let client = PolicySpatialClient::acquire()?;
        let (cur_state, fmt_ptr) = client.query(wide.as_ptr())?;
        if !validate_state_encoding(&cur_state) {
            CoTaskMemFree(Some(fmt_ptr as *const c_void));
            return Err("当前系统版本的空间音效接口布局不受支持".to_string());
        }
        let mut new_state = [0u8; SPATIAL_STATE_LEN];
        if let Some(bytes) = &guid_hex {
            new_state[0] = 1;
            new_state[4] = 1;
            new_state[8] = 1;
            new_state[0x0C..0x1C].copy_from_slice(bytes);
            new_state[0x1C..0x2C].copy_from_slice(bytes);
            new_state[0x3C] = 1;
            new_state[0x44] = 1;
        } // 关 = 全零
        if let Err(e) = client.set_state(wide.as_ptr(), &new_state, fmt_ptr) {
            CoTaskMemFree(Some(fmt_ptr as *const c_void));
            return Err(e);
        }
        CoTaskMemFree(Some(fmt_ptr as *const c_void));
        // 读回校验：核对语义字段，防止未来构建槽位漂移导致误写
        let (after, fmt_ptr2) = client.query(wide.as_ptr())?;
        CoTaskMemFree(Some(fmt_ptr2 as *const c_void));
        if !state_matches(&after, guid_hex.is_some(), guid_hex.as_ref()) {
            return Err("设置未生效（接口布局可能已变化）".to_string());
        }
        Ok(())
    }
}

/// CPolicyConfigClient 扩展接口封装。IID 随 Windows 构建漂移，按候选顺序 QI 命中即用。
struct PolicySpatialClient { ptr: *mut c_void }

impl Drop for PolicySpatialClient {
    fn drop(&mut self) {
        unsafe {
            let vt = *(self.ptr as *const *const usize);
            let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vt.add(2));
            release_fn(self.ptr);
        }
    }
}

impl PolicySpatialClient {
    const CLSID_POLICY_CONFIG_CLIENT: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0x870af99c, data2: 0x171d, data3: 0x4f9e,
        data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
    };

    unsafe fn acquire() -> std::result::Result<Self, String> {
        ensure_com_initialized();
        let iid_unknown = windows_sys::core::GUID {
            data1: 0x00000000, data2: 0x0000, data3: 0x0000,
            data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
        };
        let mut unk: *mut c_void = ptr::null_mut();
        let hr = windows_sys::Win32::System::Com::CoCreateInstance(
            &Self::CLSID_POLICY_CONFIG_CLIENT, ptr::null_mut(),
            windows_sys::Win32::System::Com::CLSCTX_ALL,
            &iid_unknown, &mut unk,
        );
        if hr < 0 || unk.is_null() {
            return Err("无法创建音频策略配置对象".to_string());
        }
        // 扩展接口 IID 候选：
        // - 4495581A：Win11 24H2 实测（slot22 单声道 / slot34/35 空间音效读写）
        // - E8478600 ：社区逆向记录的 Insider 构建变体
        // 布局自检由 query 后的 validate_state_encoding 把关，不匹配则拒绝写入。
        let candidates = [
            windows_sys::core::GUID {
                data1: 0x4495581a, data2: 0x01b9, data3: 0x4a8f,
                data4: [0xb0, 0x5c, 0x74, 0x1a, 0x6c, 0x98, 0x3d, 0x28],
            },
            windows_sys::core::GUID {
                data1: 0xe8478600, data2: 0xa74b, data3: 0x4b3a,
                data4: [0xa9, 0x6b, 0x1f, 0xc3, 0xe7, 0x96, 0xfc, 0x46],
            },
        ];
        let unk_vt = *(unk as *const *const usize);
        let qi_fn: unsafe extern "system" fn(*mut c_void, *const windows_sys::core::GUID, *mut *mut c_void) -> i32 =
            std::mem::transmute(*unk_vt);
        for iid in candidates.iter() {
            let mut out: *mut c_void = ptr::null_mut();
            let hr = qi_fn(unk, iid, &mut out);
            if hr >= 0 && !out.is_null() {
                let release_unk: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(*unk_vt.add(2));
                release_unk(unk);
                return Ok(Self { ptr: out });
            }
        }
        let release_unk: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*unk_vt.add(2));
        release_unk(unk);
        Err("当前系统不支持空间音效控制接口".to_string())
    }

    /// slot34：GetDeviceFormatAndSpatialSettings → (状态块前 0x48 字节, WAVEFORMATEX*)
    /// fmt 指针所有权移交调用方（需 CoTaskMemFree），settings/desc 内部释放
    unsafe fn query(&self, wide_ptr: *const u16) -> std::result::Result<([u8; SPATIAL_STATE_LEN], *mut u16), String> {
        let vt = *(self.ptr as *const *const usize);
        let get_fn: unsafe extern "system" fn(
            *mut c_void, *const u16, i32,
            *mut *mut u16, *mut *mut u8, *mut u32, *mut *mut u8,
        ) -> i32 = std::mem::transmute(*vt.add(34));
        let mut fmt: *mut u16 = ptr::null_mut();
        let mut settings: *mut u8 = ptr::null_mut();
        let mut mode: u32 = 0;
        let mut desc: *mut u8 = ptr::null_mut();
        let hr = get_fn(self.ptr, wide_ptr, 0, &mut fmt, &mut settings, &mut mode, &mut desc);
        if hr < 0 || settings.is_null() {
            if !fmt.is_null() { CoTaskMemFree(Some(fmt as *const c_void)); }
            if !desc.is_null() { CoTaskMemFree(Some(desc as *const c_void)); }
            return Err(format!("读取空间音效状态失败（hr={:#010x}），设备可能不支持", hr as u32));
        }
        let mut state = [0u8; SPATIAL_STATE_LEN];
        ptr::copy_nonoverlapping(settings, state.as_mut_ptr(), SPATIAL_STATE_LEN);
        CoTaskMemFree(Some(settings as *const c_void));
        CoTaskMemFree(Some(desc as *const c_void));
        Ok((state, fmt))
    }

    /// slot35：SetDeviceSpatialSettings
    unsafe fn set_state(&self, wide_ptr: *const u16, state: &[u8; SPATIAL_STATE_LEN], fmt: *const u16) -> std::result::Result<(), String> {
        let vt = *(self.ptr as *const *const usize);
        let set_fn: unsafe extern "system" fn(*mut c_void, *const u16, *const u8, *const u16) -> i32 =
            std::mem::transmute(*vt.add(35));
        let hr = set_fn(self.ptr, wide_ptr, state.as_ptr(), fmt);
        if hr < 0 {
            return Err(format!("设置空间音效失败（hr={:#010x}）", hr as u32));
        }
        Ok(())
    }
}

/// 状态编码校验：前三个 DWORD 全零（关）或全一（开），且尾部标志位取值合法。
/// 用于拦截未来构建槽位漂移导致的错误调用。
fn validate_state_encoding(state: &[u8; SPATIAL_STATE_LEN]) -> bool {
    let d = |o: usize| u32::from_le_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]]);
    let off = d(0) == 0 && d(4) == 0 && d(8) == 0;
    let on = d(0) == 1 && d(4) == 1 && d(8) == 1;
    (off || on) && d(0x3C) <= 1 && d(0x40) == 0 && d(0x44) <= 1
}

fn state_current_guid(state: &[u8; SPATIAL_STATE_LEN]) -> Option<String> {
    if state[0] == 0 && state[4] == 0 && state[8] == 0 {
        return None;
    }
    Some(format_guid_bytes(&state[0x0C..0x1C].try_into().ok()?))
}

/// 读回校验：仅核对语义字段（使能标志 + 格式 GUID），容忍尾部字段变化
fn state_matches(state: &[u8; SPATIAL_STATE_LEN], enabled: bool, guid: Option<&[u8; 16]>) -> bool {
    let d = |o: usize| u32::from_le_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]]);
    if !enabled {
        return d(0) == 0 && d(4) == 0 && d(8) == 0;
    }
    let Some(g) = guid else { return false };
    d(0) == 1 && d(4) == 1 && d(8) == 1
        && state[0x0C..0x1C] == g[..]
        && d(0x3C) == 1
        && d(0x44) == 1
}

fn format_guid_bytes(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// 解析 "{...}" 或裸十六进制 GUID 字符串为内存序字节
fn parse_guid_str(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 { return None; }
    let byte = |i: usize| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok();
    let mut raw = [0u8; 16];
    for (i, b) in raw.iter_mut().enumerate() {
        *b = byte(i)?;
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    out[4..6].copy_from_slice(&[raw[5], raw[4]]);
    out[6..8].copy_from_slice(&[raw[7], raw[6]]);
    out[8..16].copy_from_slice(&raw[8..16]);
    Some(out)
}

#[cfg(test)]
mod spatial_tests {
    use super::*;

    #[test]
    fn guid_parse_format_roundtrip() {
        for (guid, _, _) in SPATIAL_SOUND_FORMATS {
            let bytes = parse_guid_str(guid).expect("parse");
            assert_eq!(format_guid_bytes(&bytes), *guid);
        }
        assert!(parse_guid_str("").is_none());
        assert!(parse_guid_str("zzzz").is_none());
    }

    #[test]
    fn validate_and_match_states() {
        let off = [0u8; SPATIAL_STATE_LEN];
        assert!(validate_state_encoding(&off));

        let mut on = [0u8; SPATIAL_STATE_LEN];
        on[0] = 1;
        on[4] = 1;
        on[8] = 1;
        let g0 = parse_guid_str(SPATIAL_SOUND_FORMATS[0].0).unwrap();
        on[0x0C..0x1C].copy_from_slice(&g0);
        on[0x1C..0x2C].copy_from_slice(&g0);
        on[0x3C] = 1;
        on[0x44] = 1;
        assert!(validate_state_encoding(&on));

        let mut corrupted = on;
        corrupted[10] = 7;
        assert!(!validate_state_encoding(&corrupted));

        assert!(state_matches(&off, false, None));
        assert!(!state_matches(&off, true, None));
        assert!(state_matches(&on, true, Some(&g0)));
        assert!(!state_matches(&on, true, Some(&parse_guid_str(SPATIAL_SOUND_FORMATS[2].0).unwrap())));
    }
}

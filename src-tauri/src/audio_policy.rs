//! 音频策略接口：默认设备切换（IPolicyConfig）与应用级设备路由
//! （IAudioPolicyConfigFactory / SetPersistedDefaultAudioEndpoint）。
//! 与 audio.rs 的设备枚举/音量会话逻辑分离，聚焦策略层 COM/WinRT 调用。

use std::ffi::c_void;
use std::ptr;
use windows::core::*;
use windows::Win32::Media::Audio::*;

/// 音频策略配置对象的 COM CLSID（IPolicyConfig / IAudioPolicyConfigFactory 共用）
pub(crate) const CLSID_POLICY_CONFIG: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x870af99c,
    data2: 0x171d,
    data3: 0x4f9e,
    data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
};

/// IUnknown 接口 IID
pub(crate) const IID_IUNKNOWN: windows_sys::core::GUID = windows_sys::core::GUID {
    data1: 0x00000000,
    data2: 0x0000,
    data3: 0x0000,
    data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
};

pub fn set_default_device(device_id: &str) -> Result<()> {
    crate::process::append_log(&format!("[audio] set_default_device: {}", device_id));
    unsafe {
        crate::audio::ensure_com_initialized();
        let wide: Vec<u16> = crate::process::to_wide(device_id);
        set_default_device_raw(wide.as_ptr())?;
        Ok(())
    }
}

unsafe fn set_default_device_raw(wide_ptr: *const u16) -> Result<()> {
    let ipolicy_iid = windows_sys::core::GUID {
        data1: 0xf8679f50,
        data2: 0x850a,
        data3: 0x41cf,
        data4: [0x9c, 0x72, 0x43, 0x0f, 0x29, 0x02, 0x90, 0xc8],
    };

    let mut unknown_ptr: *mut c_void = ptr::null_mut();
    let hr = windows_sys::Win32::System::Com::CoCreateInstance(
        &CLSID_POLICY_CONFIG,
        ptr::null_mut(),
        windows_sys::Win32::System::Com::CLSCTX_ALL,
        &IID_IUNKNOWN,
        &mut unknown_ptr as *mut *mut _,
    );
    if hr < 0 {
        return Err(Error::empty());
    }

    let mut policy_ptr: *mut c_void = ptr::null_mut();
    let unknown_vtable = *(unknown_ptr as *const *const usize);
    let qi_fn: unsafe extern "system" fn(
        *mut c_void,
        *const windows_sys::core::GUID,
        *mut *mut c_void,
    ) -> i32 = std::mem::transmute(*unknown_vtable);
    let qi_hr = qi_fn(unknown_ptr, &ipolicy_iid, &mut policy_ptr);

    let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*(unknown_vtable.offset(2)));
    release_fn(unknown_ptr);

    if qi_hr < 0 || policy_ptr.is_null() {
        return Err(Error::empty());
    }

    let policy_vtable = *(policy_ptr as *const *const usize);
    let set_endpoint_fn: unsafe extern "system" fn(*mut c_void, PCWSTR, i32) -> i32 =
        std::mem::transmute(*policy_vtable.add(13));

    let mut all_ok = true;
    for role in 0..=2 {
        let hr = set_endpoint_fn(policy_ptr, PCWSTR(wide_ptr), role);
        if hr < 0 {
            all_ok = false;
        }
    }

    let release_fn2: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*(policy_vtable.offset(2)));
    release_fn2(policy_ptr);

    if !all_ok {
        return Err(Error::empty());
    }
    Ok(())
}

fn combase_module() -> *mut c_void {
    static COM_BASE: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    let m = *COM_BASE
        .get_or_init(|| unsafe { crate::process::load_library(b"combase.dll\0") as usize });
    m as *mut c_void
}

unsafe fn combase_proc(name: &[u8]) -> *mut c_void {
    crate::process::get_proc_address(combase_module(), name)
}

type RoGetActivationFactoryFn = unsafe extern "system" fn(
    *const c_void,
    *const windows_sys::core::GUID,
    *mut *mut c_void,
) -> i32;
type WindowsGetStringRawBufferFn = unsafe extern "system" fn(*const c_void, *mut u32) -> *const u16;
type WindowsDeleteStringFn = unsafe extern "system" fn(*const c_void) -> i32;

fn policy_config_factory_iid() -> windows_sys::core::GUID {
    windows_sys::core::GUID {
        data1: 0xab3d4648,
        data2: 0xe242,
        data3: 0x459f,
        data4: [0xb0, 0x2f, 0x54, 0x1c, 0x70, 0x30, 0x63, 0x24],
    }
}

/// 通过 WinRT 激活工厂获取 IAudioPolicyConfigFactory（Win11 21H2+ 使用 AB3D4648 变体）
unsafe fn create_policy_config_factory() -> Result<*mut c_void> {
    crate::audio::ensure_com_initialized();
    let roget_ptr = combase_proc(b"RoGetActivationFactory\0");
    if roget_ptr.is_null() {
        return Err(Error::from_hresult(HRESULT(0x80004003u32 as i32)));
    }
    let roget: RoGetActivationFactoryFn = std::mem::transmute(roget_ptr);

    let class_name = HSTRING::from("Windows.Media.Internal.AudioPolicyConfig");
    let class_raw: *const c_void = std::mem::transmute_copy::<HSTRING, *const c_void>(&class_name);
    let iid = policy_config_factory_iid();
    let mut factory: *mut c_void = ptr::null_mut();
    let hr = roget(class_raw, &iid, &mut factory);
    if hr < 0 || factory.is_null() {
        return Err(Error::from_hresult(HRESULT(hr)));
    }
    Ok(factory)
}

unsafe fn release_com_pointer(ptr: *mut c_void) {
    if ptr.is_null() {
        return;
    }
    let vtable = *(ptr as *const *const usize);
    let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
        std::mem::transmute(*vtable.add(2));
    release_fn(ptr);
}

/// 释放并读取 WinRT HSTRING 句柄的内容
unsafe fn read_hstring(handle: *const c_void) -> String {
    if handle.is_null() {
        return String::new();
    }
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
    if direction == "input" {
        eCapture
    } else {
        eRender
    }
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
    let suffix = if flow == eCapture {
        DEVINTERFACE_AUDIO_CAPTURE
    } else {
        DEVINTERFACE_AUDIO_RENDER
    };
    format!("{}{}{}", MMDEVAPI_PREFIX, device_id, suffix)
}

/// 将策略 API 返回的设备接口路径解包为 MMDevice ID
fn unpack_device_id(packed: &str) -> String {
    let mut s = packed.to_string();
    if s.starts_with(MMDEVAPI_PREFIX) {
        s = s[MMDEVAPI_PREFIX.len()..].to_string();
    }
    for suf in [DEVINTERFACE_AUDIO_RENDER, DEVINTERFACE_AUDIO_CAPTURE] {
        if s.ends_with(suf) {
            s = s[..s.len() - suf.len()].to_string();
            break;
        }
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
        let get_fn: unsafe extern "system" fn(
            *mut c_void,
            u32,
            i32,
            i32,
            *mut *const c_void,
        ) -> i32 = std::mem::transmute(*vtable.add(26));
        let mut out: *const c_void = ptr::null();
        let hr = get_fn(factory, pid, flow.0, eMultimedia.0, &mut out);
        release_com_pointer(factory);
        if hr < 0 {
            return Ok(None);
        } // 无覆盖/进程无音频
        if out.is_null() {
            return Ok(None);
        }
        let id = read_hstring(out);
        if id.is_empty() {
            Ok(None)
        } else {
            Ok(Some(unpack_device_id(&id)))
        }
    }
}

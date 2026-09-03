use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::Emitter;
use windows::core::*;
use windows::Win32::Foundation::*;
use windows::Win32::Media::Audio::Endpoints::*;
use windows::Win32::Media::Audio::*;
use windows::Win32::System::Com::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows_core::implement;

use crate::audio::{pwstr_to_string, VolumeChangeEvent};

const WM_SYNC_CALLBACKS: u32 = 0x0400;
const WM_SYNC_SESSIONS: u32 = 0x0401;

/// 属性变更节流：同一设备 2s 内只记录一次，避免日志噪音
static LAST_PROP_LOG: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

fn log_throttle_property(id: &str) {
    let lock = LAST_PROP_LOG.get_or_init(|| Mutex::new(HashMap::new()));
    let mut map = lock.lock().unwrap();
    let now = Instant::now();
    if let Some(last) = map.get(id) {
        if now.duration_since(*last) < Duration::from_secs(2) {
            return;
        }
    }
    map.insert(id.to_string(), now);
    crate::process::append_verbose_log(&format!("[audio_notify] OnPropertyValueChanged id={}", id));
}

/// 音频通知消息窗口句柄（STA 线程创建后写入，供外部线程按需投递会话同步请求）。
/// 存 isize 而非 HWND：裸指针非 Send，无法放入 static OnceLock。
static NOTIFY_HWND: OnceLock<isize> = OnceLock::new();

// ── 音量回调实现 ──────────────────────────────────────────

#[implement(IAudioEndpointVolumeCallback)]
struct VolumeCallback {
    app_handle: tauri::AppHandle,
    device_id: Arc<str>,
}

impl IAudioEndpointVolumeCallback_Impl for VolumeCallback_Impl {
    fn OnNotify(&self, pnotify: *mut AUDIO_VOLUME_NOTIFICATION_DATA) -> Result<()> {
        unsafe {
            if let Some(data) = pnotify.as_ref() {
                crate::process::append_verbose_log(&format!(
                    "[audio_notify] OnNotify 设备: {} vol={} muted={}",
                    self.device_id,
                    data.fMasterVolume,
                    data.bMuted.as_bool()
                ));
                if let Err(e) = self.app_handle.emit(
                    "volume-changed",
                    vec![VolumeChangeEvent {
                        device_id: Some(self.device_id.to_string()),
                        session_id: None,
                        volume: data.fMasterVolume,
                        is_muted: data.bMuted.as_bool(),
                    }],
                ) {
                    crate::process::append_log(&format!(
                        "[audio_notify] emit volume-changed 失败: {}",
                        e
                    ));
                }
            }
        }
        Ok(())
    }
}

// ── 会话音量回调实现（IAudioSessionEvents）────────────────

#[implement(IAudioSessionEvents)]
struct SessionVolumeCallback {
    app_handle: tauri::AppHandle,
    session_id: Arc<str>,
}

impl IAudioSessionEvents_Impl for SessionVolumeCallback_Impl {
    fn OnDisplayNameChanged(
        &self,
        _newdisplayname: &PCWSTR,
        _eventcontext: *const GUID,
    ) -> Result<()> {
        Ok(())
    }

    fn OnIconPathChanged(&self, _newiconpath: &PCWSTR, _eventcontext: *const GUID) -> Result<()> {
        Ok(())
    }

    fn OnSimpleVolumeChanged(
        &self,
        newvolume: f32,
        newmute: BOOL,
        _eventcontext: *const GUID,
    ) -> Result<()> {
        crate::process::append_verbose_log(&format!(
            "[audio_notify] OnSimpleVolumeChanged 会话: {} vol={} muted={}",
            self.session_id,
            newvolume,
            newmute.as_bool()
        ));
        if let Err(e) = self.app_handle.emit(
            "volume-changed",
            vec![VolumeChangeEvent {
                device_id: None,
                session_id: Some(self.session_id.to_string()),
                volume: newvolume,
                is_muted: newmute.as_bool(),
            }],
        ) {
            crate::process::append_log(&format!("[audio_notify] emit volume-changed 失败: {}", e));
        }
        Ok(())
    }

    fn OnChannelVolumeChanged(
        &self,
        _channelcount: u32,
        _newchannelvolumearray: *const f32,
        _changedchannel: u32,
        _eventcontext: *const GUID,
    ) -> Result<()> {
        Ok(())
    }

    fn OnGroupingParamChanged(
        &self,
        _newgroupingparam: *const GUID,
        _eventcontext: *const GUID,
    ) -> Result<()> {
        Ok(())
    }

    fn OnStateChanged(&self, _newstate: AudioSessionState) -> Result<()> {
        Ok(())
    }

    fn OnSessionDisconnected(&self, _disconnectreason: AudioSessionDisconnectReason) -> Result<()> {
        Ok(())
    }
}

// ── 设备通知回调（IMMNotificationClient）──────────────────

#[implement(IMMNotificationClient)]
struct DeviceNotification {
    hwnd: HWND,
}

impl IMMNotificationClient_Impl for DeviceNotification_Impl {
    fn OnDeviceStateChanged(&self, pwstrdeviceid: &PCWSTR, dwnewstate: DEVICE_STATE) -> Result<()> {
        unsafe {
            crate::process::append_verbose_log(&format!(
                "[audio_notify] OnDeviceStateChanged id={} state={}",
                (*pwstrdeviceid).to_string().unwrap_or_default(),
                dwnewstate.0
            ));
            let _ = PostMessageW(Some(self.hwnd), WM_SYNC_CALLBACKS, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        unsafe {
            crate::process::append_verbose_log(&format!(
                "[audio_notify] OnDeviceAdded id={}",
                (*pwstrdeviceid).to_string().unwrap_or_default()
            ));
            let _ = PostMessageW(Some(self.hwnd), WM_SYNC_CALLBACKS, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        unsafe {
            crate::process::append_verbose_log(&format!(
                "[audio_notify] OnDeviceRemoved id={}",
                (*pwstrdeviceid).to_string().unwrap_or_default()
            ));
            let _ = PostMessageW(Some(self.hwnd), WM_SYNC_CALLBACKS, WPARAM(0), LPARAM(0));
        }
        Ok(())
    }

    fn OnDefaultDeviceChanged(
        &self,
        edflow: EDataFlow,
        erender: ERole,
        pwstrdefaultdeviceid: &PCWSTR,
    ) -> Result<()> {
        unsafe {
            crate::process::append_verbose_log(&format!(
                "[audio_notify] OnDefaultDeviceChanged flow={} role={} id={}",
                edflow.0,
                erender.0,
                (*pwstrdefaultdeviceid).to_string().unwrap_or_default()
            ));
        }
        Ok(())
    }

    fn OnPropertyValueChanged(&self, pwstrdeviceid: &PCWSTR, _key: &PROPERTYKEY) -> Result<()> {
        log_throttle_property(&unsafe { (*pwstrdeviceid).to_string().unwrap_or_default() });
        Ok(())
    }
}

// ── 音频监控器 ───────────────────────────────────────────

struct AudioMonitor {
    enumerator: IMMDeviceEnumerator,
    callbacks: HashMap<String, (IAudioEndpointVolume, IAudioEndpointVolumeCallback)>,
    session_callbacks: HashMap<String, (IAudioSessionControl, IAudioSessionEvents)>,
    notification: IMMNotificationClient,
    app_handle: tauri::AppHandle,
}

impl Drop for AudioMonitor {
    fn drop(&mut self) {
        unsafe {
            let _ = self
                .enumerator
                .UnregisterEndpointNotificationCallback(&self.notification);
            for (_, (endpoint, callback)) in self.callbacks.drain() {
                let _ = endpoint.UnregisterControlChangeNotify(&callback);
            }
            for (_, (control, callback)) in self.session_callbacks.drain() {
                let _ = control.UnregisterAudioSessionNotification(&callback);
            }
        }
    }
}

impl AudioMonitor {
    fn new(hwnd: HWND, app_handle: tauri::AppHandle) -> Result<Self> {
        unsafe {
            let enumerator: IMMDeviceEnumerator =
                CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;

            let notification: IMMNotificationClient = DeviceNotification { hwnd }.into();
            enumerator.RegisterEndpointNotificationCallback(&notification)?;

            Ok(Self {
                enumerator,
                callbacks: HashMap::new(),
                session_callbacks: HashMap::new(),
                notification,
                app_handle,
            })
        }
    }

    fn sync_callbacks(&mut self) {
        unsafe {
            let collection = match self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            {
                Ok(c) => c,
                Err(_) => return,
            };

            let count = collection.GetCount().unwrap_or(0);
            let mut current_ids = Vec::with_capacity(count as usize);

            for i in 0..count {
                if let Ok(device) = collection.Item(i) {
                    if let Ok(id) = device.GetId() {
                        let id_str = pwstr_to_string(id).unwrap_or_default();
                        current_ids.push(id_str.clone());

                        if !self.callbacks.contains_key(&id_str) {
                            self.register_device(&device, &id_str);
                        }
                    }
                }
            }

            let to_remove: Vec<String> = self
                .callbacks
                .keys()
                .filter(|id| !current_ids.contains(id))
                .cloned()
                .collect();
            for id in to_remove {
                if let Some((endpoint, callback)) = self.callbacks.remove(&id) {
                    let _ = endpoint.UnregisterControlChangeNotify(&callback);
                }
            }

            self.sync_session_callbacks();

            crate::process::append_verbose_log(&format!(
                "[audio_notify] sync_callbacks: 枚举 {} 台设备，设备回调 {} 个，会话回调 {} 个",
                count,
                self.callbacks.len(),
                self.session_callbacks.len()
            ));

            if let Err(e) = self.app_handle.emit("audio-devices-changed", ()) {
                crate::process::append_log(&format!(
                    "[audio_notify] emit audio-devices-changed 失败: {}",
                    e
                ));
            }
        }
    }

    /// 枚举所有活动输出设备上的会话，注册/注销会话音量回调（会话增删无推送，靠定时重同步）
    fn sync_session_callbacks(&mut self) {
        unsafe {
            let collection = match self
                .enumerator
                .EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE)
            {
                Ok(c) => c,
                Err(_) => return,
            };

            let count = collection.GetCount().unwrap_or(0);
            let mut current_ids: Vec<String> = Vec::new();

            for i in 0..count {
                if let Ok(device) = collection.Item(i) {
                    let session_manager: IAudioSessionManager2 =
                        match device.Activate(CLSCTX_ALL, None) {
                            Ok(m) => m,
                            Err(_) => continue,
                        };
                    let session_enumerator = match session_manager.GetSessionEnumerator() {
                        Ok(e) => e,
                        Err(_) => continue,
                    };
                    let s_count = session_enumerator.GetCount().unwrap_or(0);
                    for j in 0..s_count {
                        if let Ok(session_control) = session_enumerator.GetSession(j) {
                            let session_control2: IAudioSessionControl2 =
                                match session_control.cast() {
                                    Ok(s) => s,
                                    Err(_) => continue,
                                };
                            let state = session_control2.GetState().unwrap_or(AudioSessionState(0));
                            if state.0 > 2 {
                                continue;
                            }
                            if session_control2.GetProcessId().unwrap_or(0) == 0 {
                                continue;
                            }
                            let session_id = match session_control2.GetSessionInstanceIdentifier() {
                                Ok(id) => match pwstr_to_string(id) {
                                    Ok(s) => s,
                                    Err(_) => continue,
                                },
                                Err(_) => continue,
                            };
                            current_ids.push(session_id.clone());
                            if !self.session_callbacks.contains_key(&session_id) {
                                self.register_session(&session_control, &session_id);
                            }
                        }
                    }
                }
            }

            let to_remove: Vec<String> = self
                .session_callbacks
                .keys()
                .filter(|id| !current_ids.contains(id))
                .cloned()
                .collect();
            for id in to_remove {
                if let Some((control, callback)) = self.session_callbacks.remove(&id) {
                    let _ = control.UnregisterAudioSessionNotification(&callback);
                }
            }
        }
    }

    unsafe fn register_session(&mut self, control: &IAudioSessionControl, id: &str) {
        let session_id: Arc<str> = Arc::from(id);
        let callback: IAudioSessionEvents = SessionVolumeCallback {
            app_handle: self.app_handle.clone(),
            session_id: session_id.clone(),
        }
        .into();

        match control.RegisterAudioSessionNotification(&callback) {
            Ok(()) => {
                self.session_callbacks
                    .insert(id.to_string(), (control.clone(), callback));
                crate::process::append_log(&format!(
                    "[audio_notify] registered session volume callback: {}",
                    id
                ));
            }
            Err(e) => {
                crate::process::append_log(&format!(
                    "[audio_notify] RegisterAudioSessionNotification failed: {} {}",
                    id, e
                ));
            }
        }
    }

    unsafe fn register_device(&mut self, device: &IMMDevice, id: &str) {
        let endpoint: IAudioEndpointVolume = match device.Activate(CLSCTX_ALL, None) {
            Ok(e) => e,
            Err(e) => {
                crate::process::append_log(&format!(
                    "[audio_notify] register_device Activate 失败: {} {}",
                    id, e
                ));
                return;
            }
        };

        let device_id: Arc<str> = Arc::from(id);
        let callback: IAudioEndpointVolumeCallback = VolumeCallback {
            app_handle: self.app_handle.clone(),
            device_id: device_id.clone(),
        }
        .into();

        match endpoint.RegisterControlChangeNotify(&callback) {
            Ok(()) => {
                self.callbacks.insert(id.to_string(), (endpoint, callback));
                crate::process::append_log(&format!("[audio_notify] 已注册设备音量回调: {}", id));
            }
            Err(e) => {
                crate::process::append_log(&format!(
                    "[audio_notify] RegisterControlChangeNotify 失败: {} {}",
                    id, e
                ));
            }
        }
    }
}

// ── STA 线程 ─────────────────────────────────────────────

pub fn init_audio_notify(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || unsafe {
        let hr = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        if hr.is_err() {
            crate::process::append_log("[audio_notify] CoInitializeEx failed");
            return;
        }

        let class_name: Vec<u16> = "AudioNotifyMsgWindow\0".encode_utf16().collect();
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(audio_msg_wnd_proc),
            hInstance: HINSTANCE(std::ptr::null_mut()),
            lpszClassName: PCWSTR(class_name.as_ptr()),
            ..std::mem::zeroed()
        };
        RegisterClassExW(&wc);

        let hwnd = match CreateWindowExW(
            WS_EX_TOOLWINDOW,
            PCWSTR(class_name.as_ptr()),
            PCWSTR::null(),
            WINDOW_STYLE::default(),
            0,
            0,
            0,
            0,
            None,
            None,
            Some(HINSTANCE(std::ptr::null_mut())),
            None,
        ) {
            Ok(h) => h,
            Err(_) => {
                crate::process::append_log("[audio_notify] CreateWindowExW failed");
                return;
            }
        };
        NOTIFY_HWND.set(hwnd.0 as isize).ok();

        let mut monitor = match AudioMonitor::new(hwnd, app_handle) {
            Ok(m) => m,
            Err(e) => {
                crate::process::append_log(&format!(
                    "[audio_notify] AudioMonitor::new failed: {}",
                    e
                ));
                return;
            }
        };
        monitor.sync_callbacks();

        let monitor_ptr = Box::leak(Box::new(monitor));
        SetWindowLongPtrW(
            hwnd,
            GWLP_USERDATA,
            monitor_ptr as *mut AudioMonitor as isize,
        );

        crate::process::append_log("[audio_notify] STA thread started");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, Some(hwnd), 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        crate::process::append_log("[audio_notify] STA thread stopped");
    });
}

/// 按需触发会话音量回调同步：音量控制页渲染会话音量前调用（如 get_audio_sessions 命令），
/// 投递 WM_SYNC_SESSIONS 到 STA 线程，确保当前会话的 IAudioSessionEvents 回调已注册，
/// 使会话音量变化能实时推送 volume-changed。会话本身无增删系统推送，故仅在需要时主动同步。
pub fn request_session_sync() {
    let Some(&hwnd) = NOTIFY_HWND.get() else {
        return;
    };
    if hwnd == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_SYNC_SESSIONS,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

/// 退出前投递 WM_CLOSE：触发消息窗口销毁 → WM_DESTROY → drop AudioMonitor
/// （反注册 IMMNotificationClient 与各设备/会话回调）→ STA 线程退出。
/// best-effort：仅覆盖正常退出路径（app.exit）；watchdog/panic 的 process::exit 不触发。
pub fn request_shutdown() {
    let Some(&hwnd) = NOTIFY_HWND.get() else {
        return;
    };
    if hwnd == 0 {
        return;
    }
    unsafe {
        let _ = PostMessageW(
            Some(HWND(hwnd as *mut core::ffi::c_void)),
            WM_CLOSE,
            WPARAM(0),
            LPARAM(0),
        );
    }
}

extern "system" fn audio_msg_wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    unsafe {
        match msg {
            WM_SYNC_CALLBACKS => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if ptr != 0 {
                    let monitor = &mut *(ptr as *mut AudioMonitor);
                    monitor.sync_callbacks();
                }
                LRESULT(0)
            }
            WM_SYNC_SESSIONS => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if ptr != 0 {
                    let monitor = &mut *(ptr as *mut AudioMonitor);
                    monitor.sync_session_callbacks();
                }
                LRESULT(0)
            }
            WM_CLOSE => {
                // 退出路径：销毁消息窗口 → WM_DESTROY → drop AudioMonitor（反注册回调）→ 线程退出
                let _ = DestroyWindow(hwnd);
                LRESULT(0)
            }
            WM_ENDSESSION => {
                crate::process::append_log(&format!(
                    "[audio_notify] WM_ENDSESSION received, wparam={}",
                    wparam.0
                ));
                if wparam.0 != 0 {
                    let (enabled, devices) = crate::config::with_config(|c| {
                        (c.shutdown_volume_enabled, c.shutdown_volume_devices.clone())
                    });
                    crate::process::append_log(&format!(
                        "[audio_notify] shutdown config: enabled={}, devices={:?}",
                        enabled, devices
                    ));
                    if enabled && !devices.is_empty() {
                        crate::process::append_log("[audio_notify] shutdown: adjusting volume");
                        crate::audio::set_shutdown_volumes(&devices);
                    }
                }
                LRESULT(0)
            }
            WM_DESTROY => {
                let ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA);
                if ptr != 0 {
                    drop(Box::from_raw(ptr as *mut AudioMonitor));
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}

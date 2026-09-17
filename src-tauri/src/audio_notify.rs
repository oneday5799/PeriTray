use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
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
use crate::{standard_log, verbose_log};

const WM_SYNC_CALLBACKS: u32 = 0x0400;
const WM_SYNC_SESSIONS: u32 = 0x0401;

/// 属性变更节流：同一设备 2s 内只记录一次，避免日志噪音
static LAST_PROP_LOG: OnceLock<Mutex<HashMap<String, Instant>>> = OnceLock::new();

/// WM_SYNC_CALLBACKS 合并标志：已排队则跳过，处理时复位
static SYNC_CALLBACKS_PENDING: AtomicBool = AtomicBool::new(false);

/// 投递 WM_SYNC_CALLBACKS。单独一层是为了让单测能注入「必然失败」的桩，
/// 从而直接断言失败路径的回滚行为（见文件末尾单测）。
fn post_sync_callbacks(hwnd: HWND) -> windows::core::Result<()> {
    // SAFETY: hwnd 由调用方保证是本模块消息窗口的有效句柄
    unsafe { PostMessageW(Some(hwnd), WM_SYNC_CALLBACKS, WPARAM(0), LPARAM(0)) }
}

/// 请求一次回调同步：CAS 抢占合并标志 → 投递消息。
///
/// **投递失败必须回滚标志**：标志已置 `true` 而消息未入队时，消息处理器永不运行，
/// 标志会**永久停在 `true`**，此后所有设备变更回调都在 CAS 处失败并静默跳过 ——
/// 音频设备变更通知彻底失效，且不产生任何日志。原实现写作 `let _ = PostMessageW(...)`，
/// 恰好把这个失败吞掉了（见代码审查报告 P3-12）。
///
/// 4 个 COM 回调统一走本函数，避免下次再漏改其中一处。
fn request_sync_callbacks(hwnd: HWND) {
    request_sync_callbacks_with(hwnd, post_sync_callbacks);
}

/// `request_sync_callbacks` 的可注入版本（投递动作由 `post` 提供，便于单测）。
fn request_sync_callbacks_with(hwnd: HWND, post: fn(HWND) -> windows::core::Result<()>) {
    if SYNC_CALLBACKS_PENDING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        // 已有一次同步在排队，本轮合并掉
        return;
    }
    if post(hwnd).is_err() {
        // 回滚：否则合并标志永久为 true，后续回调全部被静默跳过
        SYNC_CALLBACKS_PENDING.store(false, Ordering::SeqCst);
        verbose_log!("[audio_notify] PostMessageW(WM_SYNC_CALLBACKS) 失败，已回滚合并标志");
    }
}

fn log_throttle_property(id: &str) {
    let lock = LAST_PROP_LOG.get_or_init(|| Mutex::new(HashMap::new()));
    // 走统一入口（P2-2）：本函数由 COM 回调调用，若用 `Mutex::lock()` + `unwrap()`，
    // 锁中毒时的 panic 会穿过 FFI/COM 边界向外抛（未定义行为），
    // 且此后每次属性变更回调都会再炸一次。
    let mut map = crate::state::lock_unpoisoned(lock);
    let now = Instant::now();
    if let Some(last) = map.get(id) {
        if now.duration_since(*last) < Duration::from_secs(2) {
            return;
        }
    }
    map.insert(id.to_string(), now);
    verbose_log!("[audio_notify] OnPropertyValueChanged id={}", id);
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
                verbose_log!(
                    "[audio_notify] OnNotify 设备: {} vol={} muted={}",
                    self.device_id,
                    data.fMasterVolume,
                    data.bMuted.as_bool()
                );
                if let Err(e) = self.app_handle.emit(
                    "volume-changed",
                    vec![VolumeChangeEvent {
                        device_id: Some(self.device_id.to_string()),
                        session_id: None,
                        volume: data.fMasterVolume,
                        is_muted: data.bMuted.as_bool(),
                    }],
                ) {
                    standard_log!("[audio_notify] emit volume-changed 失败: {}", e);
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
        verbose_log!(
            "[audio_notify] OnSimpleVolumeChanged 会话: {} vol={} muted={}",
            self.session_id,
            newvolume,
            newmute.as_bool()
        );
        if let Err(e) = self.app_handle.emit(
            "volume-changed",
            vec![VolumeChangeEvent {
                device_id: None,
                session_id: Some(self.session_id.to_string()),
                volume: newvolume,
                is_muted: newmute.as_bool(),
            }],
        ) {
            standard_log!("[audio_notify] emit volume-changed 失败: {}", e);
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
            verbose_log!(
                "[audio_notify] OnDeviceStateChanged id={} state={}",
                (*pwstrdeviceid).to_string().unwrap_or_default(),
                dwnewstate.0
            );
            // 合并：已排队则跳过；投递失败由 request_sync_callbacks 回滚标志
            request_sync_callbacks(self.hwnd);
        }
        Ok(())
    }

    fn OnDeviceAdded(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        unsafe {
            verbose_log!(
                "[audio_notify] OnDeviceAdded id={}",
                (*pwstrdeviceid).to_string().unwrap_or_default()
            );
            // 合并：已排队则跳过；投递失败由 request_sync_callbacks 回滚标志
            request_sync_callbacks(self.hwnd);
        }
        Ok(())
    }

    fn OnDeviceRemoved(&self, pwstrdeviceid: &PCWSTR) -> Result<()> {
        unsafe {
            verbose_log!(
                "[audio_notify] OnDeviceRemoved id={}",
                (*pwstrdeviceid).to_string().unwrap_or_default()
            );
            // 合并：已排队则跳过；投递失败由 request_sync_callbacks 回滚标志
            request_sync_callbacks(self.hwnd);
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
            verbose_log!(
                "[audio_notify] OnDefaultDeviceChanged flow={} role={} id={}",
                edflow.0,
                erender.0,
                (*pwstrdefaultdeviceid).to_string().unwrap_or_default()
            );
            // 合并：已排队则跳过；投递失败由 request_sync_callbacks 回滚标志
            request_sync_callbacks(self.hwnd);
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

            verbose_log!(
                "[audio_notify] sync_callbacks: 枚举 {} 台设备，设备回调 {} 个，会话回调 {} 个",
                count,
                self.callbacks.len(),
                self.session_callbacks.len()
            );

            if let Err(e) = self.app_handle.emit("audio-devices-changed", ()) {
                standard_log!("[audio_notify] emit audio-devices-changed 失败: {}", e);
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
                standard_log!("[audio_notify] registered session volume callback: {}", id);
            }
            Err(e) => {
                standard_log!(
                    "[audio_notify] RegisterAudioSessionNotification failed: {} {}",
                    id,
                    e
                );
            }
        }
    }

    unsafe fn register_device(&mut self, device: &IMMDevice, id: &str) {
        let endpoint: IAudioEndpointVolume = match device.Activate(CLSCTX_ALL, None) {
            Ok(e) => e,
            Err(e) => {
                standard_log!("[audio_notify] register_device Activate 失败: {} {}", id, e);
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
                standard_log!("[audio_notify] 已注册设备音量回调: {}", id);
            }
            Err(e) => {
                standard_log!(
                    "[audio_notify] RegisterControlChangeNotify 失败: {} {}",
                    id,
                    e
                );
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
                standard_log!("[audio_notify] AudioMonitor::new failed: {}", e);
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

/// 同步版本：等待 STA 线程完成会话同步后再返回（用于 get_audio_sessions 命令）
/// 注意：必须在非 STA 线程中调用，否则 SendMessageW 会死锁
pub fn request_session_sync_blocking() {
    let Some(&hwnd) = NOTIFY_HWND.get() else {
        standard_log!("[audio_notify] request_session_sync_blocking: STA 线程未就绪，跳过会话同步");
        return;
    };
    if hwnd == 0 {
        standard_log!(
            "[audio_notify] request_session_sync_blocking: STA 窗口句柄为 0，跳过会话同步"
        );
        return;
    }
    unsafe {
        let _ = SendMessageW(
            HWND(hwnd as *mut core::ffi::c_void),
            WM_SYNC_SESSIONS,
            Some(WPARAM(0)),
            Some(LPARAM(0)),
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
                // 复位合并标志，允许后续消息再次排队
                SYNC_CALLBACKS_PENDING.store(false, Ordering::SeqCst);
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
                standard_log!("[audio_notify] WM_ENDSESSION received, wparam={}", wparam.0);
                if wparam.0 != 0 {
                    let (enabled, devices) = crate::config::with_config(|c| {
                        (c.shutdown_volume_enabled, c.shutdown_volume_devices.clone())
                    });
                    standard_log!(
                        "[audio_notify] shutdown config: enabled={}, devices={:?}",
                        enabled,
                        devices
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    /// P3-12 的可证伪单测：**投递失败后合并标志必须回到 `false`**。
    ///
    /// 修复前该路径写作 `let _ = PostMessageW(...)`，失败被吞掉、标志留在 `true`，
    /// 此后所有设备变更回调都在 CAS 处失败并静默跳过 —— 音频变更通知永久静默。
    /// 本测试通过注入必然失败的投递桩直接命中该路径：把回滚那一行删掉即失败。
    #[test]
    fn sync_pending_flag_rolls_back_when_post_fails() {
        let fake = HWND(std::ptr::null_mut());

        // ① 投递失败 → 标志回滚
        SYNC_CALLBACKS_PENDING.store(false, Ordering::SeqCst);
        request_sync_callbacks_with(fake, |_| Err(windows::core::Error::from(E_FAIL)));
        assert!(
            !SYNC_CALLBACKS_PENDING.load(Ordering::SeqCst),
            "投递失败后合并标志必须回到 false，否则后续回调会被永久合并掉"
        );

        // ② 投递成功 → 标志保持 true，等消息处理器复位
        request_sync_callbacks_with(fake, |_| Ok(()));
        assert!(
            SYNC_CALLBACKS_PENDING.load(Ordering::SeqCst),
            "投递成功后应保持已排队状态"
        );

        // 复原，避免影响其它测试
        SYNC_CALLBACKS_PENDING.store(false, Ordering::SeqCst);
    }
}

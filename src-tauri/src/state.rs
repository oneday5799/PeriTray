use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, MutexGuard, OnceLock};
use tauri::menu::MenuItem;

use crate::device::Device;

/// 托盘图标位置
pub static TRAY_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 弹窗窗口位置
pub static POPUP_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 托盘图标所在显示器的信息（缩放因子 + 逻辑工作区），
/// 用于混合 DPI 下弹窗定位与高度 clamp。
#[derive(Debug, Clone, Copy)]
pub struct TrayMonitorInfo {
    pub scale_factor: f64,
    pub work_x: f64,
    pub work_y: f64,
    pub work_w: f64,
    pub work_h: f64,
}

/// 托盘所在显示器信息缓存（托盘点击时刷新，None 表示尚未确定）
pub static TRAY_MONITOR: OnceLock<Mutex<Option<TrayMonitorInfo>>> = OnceLock::new();

/// 获取托盘所在显示器信息缓存的引用
pub fn get_tray_monitor() -> &'static Mutex<Option<TrayMonitorInfo>> {
    TRAY_MONITOR.get_or_init(|| Mutex::new(None))
}

/// 弹窗动画状态
pub static ANIMATING: AtomicBool = AtomicBool::new(false);

/// 开机自启状态
pub static AUTO_START: AtomicBool = AtomicBool::new(false);

/// 快捷键录制期间置位，抑制全局快捷键触发，避免录制时误触发动作
pub static SHORTCUT_RECORDING: AtomicBool = AtomicBool::new(false);

/// 开机自启菜单项引用
pub static AUTO_MENU_ITEM: OnceLock<Mutex<Option<MenuItem<tauri::Wry>>>> = OnceLock::new();

/// 容忍 Mutex 中毒的加锁：锁中毒时直接接管内部数据继续使用
/// （本项目各静态量在 panic 后仅需"可用"而非"严格一致"，统一在此表达该语义）
pub fn lock_unpoisoned<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// 单飞标志的 RAII 守卫：CAS 抢占，Drop 时复位。
/// 正常返回与 panic 展开均会复位，避免后台任务被永久锁死。
///
/// 用法要点（见 AGENTS.md「RAII 守卫的绑定命名」）：
/// - **外层绑定不得以 `_` 开头**——若守卫需要在线程闭包内被持有，必须以具名绑定
///   `let Some(guard) = ...` 取得，再在闭包体内显式引用（`let _guard = guard;`）触发捕获。
///   写成 `let Some(_guard)` 且闭包内不引用它时，`move` 闭包**不会捕获它**，
///   守卫会在函数返回时立即 Drop，单飞语义退化为无保护。
/// - 同步路径（在本函数内跑完工作再返回）用 `let Some(_guard)` 是正确的：Drop-only 绑定。
pub(crate) struct SingleFlightGuard<'a> {
    flag: &'a AtomicBool,
}

impl<'a> SingleFlightGuard<'a> {
    /// CAS 获取标志；成功返回 guard，失败返回 None（已有任务在跑）
    pub(crate) fn new(flag: &'a AtomicBool) -> Option<Self> {
        if flag
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            Some(Self { flag })
        } else {
            None
        }
    }
}

impl Drop for SingleFlightGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}

/// 设备缓存，用于托盘 tooltip 显示，避免重复 WMI 查询
static DEVICES_CACHE: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();

/// 获取设备缓存的引用
pub fn get_devices_cache() -> &'static Mutex<Vec<Device>> {
    DEVICES_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

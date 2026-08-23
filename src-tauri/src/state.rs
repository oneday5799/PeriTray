use std::sync::atomic::AtomicBool;
use std::sync::{Mutex, OnceLock};
use tauri::menu::MenuItem;

use crate::device::Device;

/// 托盘图标位置
pub static TRAY_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 弹窗窗口位置
pub static POPUP_POS: OnceLock<Mutex<(f64, f64)>> = OnceLock::new();

/// 弹窗动画状态
pub static ANIMATING: AtomicBool = AtomicBool::new(false);

/// 开机自启状态
pub static AUTO_START: AtomicBool = AtomicBool::new(false);

/// 快捷键录制期间置位，抑制全局快捷键触发，避免录制时误触发动作
pub static SHORTCUT_RECORDING: AtomicBool = AtomicBool::new(false);

/// 开机自启菜单项引用
pub static AUTO_MENU_ITEM: OnceLock<Mutex<Option<MenuItem<tauri::Wry>>>> = OnceLock::new();

/// 设备缓存，用于托盘 tooltip 显示，避免重复 WMI 查询
static DEVICES_CACHE: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();

/// 获取设备缓存的引用
pub fn get_devices_cache() -> &'static Mutex<Vec<Device>> {
    DEVICES_CACHE.get_or_init(|| Mutex::new(Vec::new()))
}

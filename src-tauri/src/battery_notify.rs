use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::config;
use crate::device::Device;
use crate::{standard_log, verbose_log};

// ── 去重状态 ──
// 每个 (设备名, 阈值) 组合只通知一次；重启后清空重新检测
static NOTIFIED: OnceLock<Mutex<HashSet<(String, i32)>>> = OnceLock::new();

/// 检查设备电量是否达到配置的阈值，命中时弹出 Windows 原生通知。
/// 由 tray watcher 每轮调用，传入设备缓存快照。
pub fn check_battery_notify(devices: &[Device]) {
    let (enabled, selected, thresholds) = config::with_config(|c| {
        (
            c.low_battery_notify,
            c.low_battery_devices.clone(),
            c.low_battery_thresholds.clone(),
        )
    });

    if !enabled || thresholds.is_empty() {
        verbose_log!(
            "[battery-notify] 跳过：enabled={}, thresholds={}",
            enabled,
            thresholds.len()
        );
        return;
    }

    let notified = NOTIFIED.get_or_init(|| Mutex::new(HashSet::new()));

    for d in devices {
        let Some(level) = d.battery else {
            verbose_log!("[battery-notify] 跳过 {}：无电量数据", d.name);
            continue;
        };

        // 未选择任何设备 → 不通知
        if selected.is_empty() {
            crate::process::append_verbose_log("[battery-notify] 跳过：未选择任何设备");
            continue;
        }

        // 指定了设备列表但当前设备不在其中 → 跳过
        if !selected.contains(&d.name) {
            verbose_log!("[battery-notify] 跳过 {}：不在选中列表", d.name);
            continue;
        }

        // 取用户自定义显示名，无则用原始名
        let display_name = config::with_config(|c| {
            c.device_names
                .get(&d.name)
                .cloned()
                .unwrap_or_else(|| d.name.clone())
        });

        for &threshold in &thresholds {
            if level <= threshold {
                // 去重：insert 返回 false 表示已存在（已通知过）
                // P2-11：原本是手写展开的 `.lock().unwrap_or_else(..)`，
                // 语义与 lock_unpoisoned 一致，统一收敛到单一入口
                if !crate::state::lock_unpoisoned(notified).insert((d.name.clone(), threshold)) {
                    verbose_log!(
                        "[battery-notify] 跳过 {}：阈值 {}% 已通知过",
                        d.name,
                        threshold
                    );
                    continue;
                }

                #[cfg(target_os = "windows")]
                {
                    // 图标解析下移到「确实要弹通知」处（P2-6）：原先在函数开头就解析，
                    // 而下面的 `selected.is_empty()` / 阈值去重都可能让整轮一条都不弹。
                    // 解析本身有 `OnceLock` 缓存，循环内重复调用只读一次内存。
                    let icon = crate::windows::resolve_toast_icon();
                    crate::toast::show_toast(
                        "低电量提醒",
                        &format!("{} 电量仅剩 {}%", display_name, level),
                        icon.as_deref(),
                    );
                }

                standard_log!(
                    "[battery-notify] {} 电量 {}% ≤ 阈值 {}%",
                    display_name,
                    level,
                    threshold
                );
            }
        }
    }
}

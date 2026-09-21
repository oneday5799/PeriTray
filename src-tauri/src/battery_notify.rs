use std::collections::HashSet;
use std::sync::{Mutex, OnceLock};

use crate::config;
use crate::device::Device;
use crate::{standard_log, verbose_log};

// ── 去重状态 ──
// 每个 (设备名, 阈值) 组合只通知一次；重启后清空重新检测
static NOTIFIED: OnceLock<Mutex<HashSet<(String, i32)>>> = OnceLock::new();

// ── P2-7 验收探针（仅测试构建存在，release 下零代码）────────────────
//
// 「发送阶段设备缓存锁已释放」这条性质**没有返回值可断言**，只能观测调用时刻的
// 锁状态，故在两个真实函数的入口各放一个探针，由
// `tests::cache_lock_is_released_before_emit` 读回。
// 取值：`-1` 未观测；`0` 观测到**锁被持有**；`1` 观测到**锁可获取**。
#[cfg(test)]
static COLLECT_LOCK_PROBE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);
#[cfg(test)]
static EMIT_LOCK_PROBE: std::sync::atomic::AtomicI8 = std::sync::atomic::AtomicI8::new(-1);

/// 记录「此刻设备缓存锁是否可获取」到指定探针（仅测试构建存在）。
///
/// ⚠️ 判据必须用 `try_lock` 而不是「再 `lock()` 一次」：后者在同线程重入时**挂死**，
/// 会把「缺陷」表现成「测试卡住」而不是「断言失败」（本仓既有教训）。
#[cfg(test)]
fn record_devices_cache_lock_state(probe: &std::sync::atomic::AtomicI8) {
    use std::sync::atomic::Ordering;
    let free = crate::state::get_devices_cache().try_lock().is_ok();
    probe.store(i8::from(free), Ordering::Relaxed);
}

/// 回差（hysteresis）：电量必须比阈值高出这么多，才把「已通知」标记清掉。
///
/// ── 为什么需要回差（P3-3）────────────────────────────────────────
/// 修复前 `notified` 只会**插入**、从不移除 ⇒ 一台设备掉到 20% 弹过一次提醒后，
/// 即便用户插上电源充到 100%、拔掉再一路掉回 20%，也**永远不会再提醒**。
/// 这个标记只有在进程重启（`NOTIFIED` 是 `OnceLock`，随进程清零）后才失效，
/// 于是「提醒过一次就永久静默」——比不提醒更糟：用户会以为功能坏了。
///
/// 但清标记不能直接写「电量 > 阈值就清」：电量在阈值附近抖动（19% ↔ 20%）
/// 时会变成「充电一下、放电一下」反复弹窗，同样扰人。留 5% 回差，
/// 让「重新武装」需要一个明确的、有意义的电量回升。
const HYSTERESIS_PERCENT: i32 = 5;

/// 判断电量是否已回升到「可以把 (设备, 阈值) 重新武装」的程度。
///
/// 边界取**严格大于** `threshold + HYSTERESIS_PERCENT`：
/// - `level <= threshold`：仍在告警区，不重新武装；
/// - `threshold < level <= threshold + HYSTERESIS_PERCENT`：**抖动带**，保持静默
///   （这正是回差存在的意义）；
/// - `level > threshold + HYSTERESIS_PERCENT`：明确回升，允许下次再提醒。
///
/// 抽成纯函数以便边界两侧都断言（项目既有范式）。
fn is_rearmed(level: i32, threshold: i32) -> bool {
    level > threshold + HYSTERESIS_PERCENT
}

/// 一条待弹出的低电量通知（**纯数据**，不含任何已解析的句柄或资源）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PendingBatteryNotice {
    /// 用户可见的显示名（已套用 `device_names` 里的自定义名）
    pub display_name: String,
    /// 当前电量
    pub level: i32,
    /// 命中的阈值
    pub threshold: i32,
}
/// 判定哪些设备该弹通知（**纯函数**，不读全局配置、不碰锁、不做 I/O）。
///
/// 抽出来的理由与项目既有范式一致（见 P2-4 的 `should_restart` / `is_time_jump`）：
/// 判定语义可以在单测里钉死，不必依赖真实配置与真实通知器。
///
/// `notified` 是「已通知过」的集合，本函数会**就地更新**它：
/// 命中即插入（跨轮去重）；电量明确回升（见 `is_rearmed`）即移除
/// （让下一次掉电还能提醒）。这是它唯一的副作用，且是纯内存的。
///
/// 参数里的 `Enabled/Selected/Thresholds/DeviceNames` 都是在**锁外**取好的快照，
/// 故本函数不可能落在任何锁的作用域里。
fn select_pending_notices(
    devices: &[Device],
    enabled: bool,
    selected: &[String],
    thresholds: &[i32],
    device_names: &std::collections::HashMap<String, String>,
    notified: &mut HashSet<(String, i32)>,
) -> Vec<PendingBatteryNotice> {
    if !enabled || thresholds.is_empty() {
        return Vec::new();
    }

    // 未选择任何设备 → 不通知
    if selected.is_empty() {
        return Vec::new();
    }

    let mut pending = Vec::new();

    for d in devices {
        let Some(level) = d.battery else {
            verbose_log!("[battery-notify] 跳过 {}：无电量数据", d.name);
            continue;
        };

        // 指定了设备列表但当前设备不在其中 → 跳过
        if !selected.contains(&d.name) {
            verbose_log!("[battery-notify] 跳过 {}：不在选中列表", d.name);
            continue;
        }

        // 取用户自定义显示名，无则用原始名
        let display_name = device_names
            .get(&d.name)
            .cloned()
            .unwrap_or_else(|| d.name.clone());

        for &threshold in thresholds {
            // 先处理「重新武装」：电量明确回升时清掉去重标记（P3-3）。
            // 必须**早于**命中判定 —— 否则同一轮里水位刚好跨过回差线时，
            // 「先判命中（清标记前已插入过）→ 再清标记」的次序会让标记
            // 被清掉却又在同一轮弹了一次，下一轮再弹一次。
            if is_rearmed(level, threshold) && notified.remove(&(d.name.clone(), threshold)) {
                verbose_log!(
                    "[battery-notify] {} 电量回升到 {}%（阈值 {}% + 回差 {}%），重新武装",
                    d.name,
                    level,
                    threshold,
                    HYSTERESIS_PERCENT
                );
            }

            if level <= threshold {
                // 去重：insert 返回 false 表示已存在（已通知过）
                if !notified.insert((d.name.clone(), threshold)) {
                    verbose_log!(
                        "[battery-notify] 跳过 {}：阈值 {}% 已通知过",
                        d.name,
                        threshold
                    );
                    continue;
                }

                pending.push(PendingBatteryNotice {
                    display_name: display_name.clone(),
                    level,
                    threshold,
                });
            }
        }
    }

    pending
}

/// 检查设备电量是否达到配置的阈值，**收集**待通知条目并返回。
///
/// ── 为什么要与「显示」拆开（P2-7）────────────────────────────────
/// 原实现是 `check_battery_notify` 一个函数做完全部工作（取配置 → 判定 →
/// `show_toast`），而它的调用点写成：
///
/// ```ignore
/// crate::battery_notify::check_battery_notify(&crate::state::lock_unpoisoned(cache));
/// ```
///
/// `lock_unpoisoned(cache)` 是**临时量**，其 `MutexGuard` 存活至**整条语句结束**
/// （Rust 临时量的生命周期是「所在语句」），于是整个 `check_battery_notify`
/// ——其中包括 `show_toast`（WinRT/COM 调用）与**循环内每台设备一次**的
/// `config::with_config`——**全程持有设备缓存锁**。
///
/// 这同时违反两条既有纪律（`AGENTS.md`「持锁区不得调用…」）：
/// ① 持锁做 **COM 调用**；
/// ② 持锁取**另一把锁**（设备缓存锁 → 配置锁），构成锁序依赖。
/// 反向路径真实存在：`update_config` 等命令在**主线程**先取配置锁，
/// 而 `refresh_devices_cache` / `apply_devices_cache` 会取设备缓存锁
/// ⇒ 与「设备缓存锁 → 配置锁」构成 AB/BA 死锁的完整条件。
///
/// 拆成 `collect_*`（纯内存判定，可由调用方在锁内调用）+ `emit_notifications`
/// （出锁后显示）后，锁的持有范围退化成「把缓存换成待通知列表」这一次调用。
pub fn collect_pending_notices(devices: &[Device]) -> Vec<PendingBatteryNotice> {
    #[cfg(test)]
    record_devices_cache_lock_state(&COLLECT_LOCK_PROBE);

    // 一次性把需要的配置全部 `clone` 出来，避免在循环里反复取配置锁。
    // 原先第 53 行的「每台设备取一次配置锁」在设备多时是 O(N) 次加锁，
    // 而 `device_names` 是同一份快照，取一次即可。
    let (enabled, selected, thresholds, device_names) = config::with_config(|c| {
        (
            c.low_battery_notify,
            c.low_battery_devices.clone(),
            c.low_battery_thresholds.clone(),
            c.device_names.clone(),
        )
    });

    if !enabled || thresholds.is_empty() {
        verbose_log!(
            "[battery-notify] 跳过：enabled={}, thresholds={}",
            enabled,
            thresholds.len()
        );
        return Vec::new();
    }

    // 未选择任何设备 → 不通知
    if selected.is_empty() {
        crate::process::append_verbose_log("[battery-notify] 跳过：未选择任何设备");
        return Vec::new();
    }

    let notified = NOTIFIED.get_or_init(|| Mutex::new(HashSet::new()));
    let mut guard = crate::state::lock_unpoisoned(notified);
    select_pending_notices(
        devices,
        enabled,
        &selected,
        &thresholds,
        &device_names,
        &mut guard,
    )
}

/// 设备缓存锁的**两段式**执行器：锁内收集、**锁外**发送（P2-7 的唯一承载点）。
///
/// ── 为什么要有这一层（P2-7）────────────────────────────────────────
/// 「锁内只收集、锁外再发通知」这条纪律原先只由**调用方的花括号**承载：
///
/// ```ignore
/// let pending = { let g = lock_unpoisoned(cache); collect_pending_notices(&g) };
/// emit_notifications(&pending);
/// ```
///
/// 一旦有人把它压回一行 `check(&lock_unpoisoned(cache))`，`guard` 就是**临时量**、
/// 存活到**整条语句结束**（本仓反复踩的坑，见 `AGENTS.md`）⇒ 通知（WinRT/COM）
/// 与图标文件 I/O 全落进锁内，并与配置锁构成 AB/BA：主线程的命令先取配置锁，
/// 而 `apply_devices_cache` 会取设备缓存锁。
///
/// 把这段结构收进本函数后，「先让 guard 落域、再发送」由**函数体**保证，
/// 不再依赖读代码的人是否细心；`emit_notifications` 同时降为**私有**，
/// 仓内不再存在「持锁调用发送」的第二处入口。
/// 该性质由 `tests::cache_lock_is_released_before_emit` 直接证伪。
pub fn notify_low_battery(cache: &Mutex<Vec<Device>>) {
    let pending = {
        let guard = crate::state::lock_unpoisoned(cache);
        collect_pending_notices(&guard)
    }; // ← 设备缓存锁在此释放，下面一行不得挪进上面的花括号
    emit_notifications(&pending);
}

/// 把待通知条目逐条弹成系统通知（**必须在释放配置锁 / 设备缓存锁之后调用**）。
///
/// 图标解析放在这里而不是 `collect_*`：`resolve_toast_icon()` 会碰文件系统
/// （首次写入 `%TEMP%`），属于「锁内不得做的 I/O」。它本身有 `OnceLock` 缓存，
/// 循环内重复调用只读一次内存（P2-6）。
///
/// ⚠️ **刻意不 `pub`**（P2-7）：本函数必须在锁外调用，而「锁外」无法用类型表达。
/// 保持私有 ⇒ 全仓只有 [`notify_low_battery`] 能调到它，而那一处的花括号
/// 已经把 guard 落域，于是「持锁发送」在结构上不可达。
fn emit_notifications(notices: &[PendingBatteryNotice]) {
    #[cfg(test)]
    record_devices_cache_lock_state(&EMIT_LOCK_PROBE);

    for n in notices {
        #[cfg(target_os = "windows")]
        {
            let icon = crate::windows::resolve_toast_icon();
            crate::toast::show_toast(
                "低电量提醒",
                &format!("{} 电量仅剩 {}%", n.display_name, n.level),
                icon.as_deref(),
            );
        }

        standard_log!(
            "[battery-notify] {} 电量 {}% ≤ 阈值 {}%",
            n.display_name,
            n.level,
            n.threshold
        );
    }
}

#[cfg(test)]
mod tests {
    use super::{
        is_rearmed, notify_low_battery, select_pending_notices, PendingBatteryNotice,
        COLLECT_LOCK_PROBE, EMIT_LOCK_PROBE,
    };
    use crate::device::{DevType, Device};
    use std::collections::{HashMap, HashSet};

    /// ★ 核心验收（P2-7）：**发送阶段设备缓存锁必须已释放**。
    ///
    /// 这条性质没有返回值可断言，只能观测调用时刻的锁状态，故由两个真实函数
    /// 入口处的探针（`COLLECT_LOCK_PROBE` / `EMIT_LOCK_PROBE`）读回。
    ///
    /// ── 三段式（缺一即可能写成恒真判据）──────────────────────────────
    /// ① **前置断言**：收集阶段必须**确实持锁**（探针 = 0）。若收集阶段压根没持锁，
    ///    下面「发送阶段锁可用」就是恒真的废话。
    /// ② **正控**：两个探针都必须被写到（`!= -1`），证明两阶段真的都执行了
    ///    —— 否则「函数没被调用」会被读成「锁已释放」。
    /// ③ **对照判据**：把 `notify_low_battery` 的 `guard` 挪到 `emit_notifications`
    ///    之前（即恢复成「持锁发送」的旧形态）后，本用例必须转红。
    ///
    /// 实得（2026-09-19）：正常形态 ①=0 / ②均被写 / ③=1 全绿；
    /// 注入「guard 活到发送处」后 ③ 报 `left: 0, right: 1` 转红。
    #[test]
    fn cache_lock_is_released_before_emit() {
        use std::sync::atomic::Ordering;

        // `collect_pending_notices` 会经 `with_config` 读配置；不初始化会 panic。
        // 取 `Config::default()`（**不读磁盘**）：其中未选任何设备 ⇒ 不会真的弹通知。
        crate::config::ensure_config_ready();

        COLLECT_LOCK_PROBE.store(-1, Ordering::Relaxed);
        EMIT_LOCK_PROBE.store(-1, Ordering::Relaxed);

        notify_low_battery(crate::state::get_devices_cache());

        // ② 正控：两阶段都必须被走到（探针被写过）
        let collect_probe = COLLECT_LOCK_PROBE.load(Ordering::Relaxed);
        let emit_probe = EMIT_LOCK_PROBE.load(Ordering::Relaxed);
        assert_ne!(
            collect_probe, -1,
            "正控失败：collect_pending_notices 未被调用，本用例无法说明任何问题"
        );
        assert_ne!(
            emit_probe, -1,
            "正控失败：emit_notifications 未被调用，本用例无法说明任何问题"
        );

        // ① 前置断言：收集阶段确实持有设备缓存锁
        assert_eq!(
            collect_probe, 0,
            "前置断言失败：收集阶段竟未持有设备缓存锁 ⇒ 本用例失去意义（先查锁是否还在）"
        );

        // ③ 被测性质：发送阶段锁已释放
        assert_eq!(
            emit_probe, 1,
            "P2-7 回归：emit_notifications 运行时仍持有设备缓存锁 —— \
             通知里的 COM 调用与图标文件 I/O 会落进锁内，并与配置锁构成 AB/BA"
        );
    }

    /// 造一台只关心名字与电量的设备（其余字段与本轮判定无关）。
    fn dev(name: &str, battery: Option<i32>) -> Device {
        Device {
            name: name.to_string(),
            dt: DevType::Battery,
            status: "OK".to_string(),
            battery,
            device_id: None,
            device_key: None,
            is_bluetooth: false,
            is_wireless_24g: false,
            is_ble: false,
        }
    }

    fn names(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect()
    }

    fn sel(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    /// 阈值判据取**闭区间**（`level <= threshold` 才算命中）。
    ///
    /// 边界两侧都断言：`= threshold` 命中、`threshold + 1` 不命中。
    /// 修复前该判据埋在 `check_battery_notify` 里（且全程持锁），无法单测；
    /// 抽成纯函数后边界才可钉死。
    #[test]
    fn threshold_boundary_is_inclusive() {
        let devices = vec![dev("耳机", Some(20)), dev("鼠标", Some(21))];
        let mut notified = HashSet::new();

        let pending = select_pending_notices(
            &devices,
            true,
            &sel(&["耳机", "鼠标"]),
            &[20],
            &HashMap::new(),
            &mut notified,
        );

        assert_eq!(pending.len(), 1, "只有恰好等于阈值的应命中: {pending:?}");
        assert_eq!(pending[0].display_name, "耳机");
        assert_eq!(pending[0].level, 20);
        assert_eq!(pending[0].threshold, 20);
    }

    /// 去重：同一个 (设备, 阈值) 只命中一次，**跨轮持久**。
    ///
    /// `notified` 由调用方持有并就地更新，第二轮必须拿到空结果——
    /// 否则用户每轮都会被同一条低电量提醒轰炸（刷新间隔只有 10~60s）。
    #[test]
    fn same_device_and_threshold_notifies_only_once() {
        let devices = vec![dev("耳机", Some(15))];
        let mut notified = HashSet::new();

        let first = select_pending_notices(
            &devices,
            true,
            &sel(&["耳机"]),
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        let second = select_pending_notices(
            &devices,
            true,
            &sel(&["耳机"]),
            &[20],
            &HashMap::new(),
            &mut notified,
        );

        assert_eq!(first.len(), 1, "首次应命中");
        assert!(second.is_empty(), "同一 (设备,阈值) 第二轮不得重复通知");
    }

    /// 多个阈值可各自命中一次；被去重的那些不影响其余阈值。
    #[test]
    fn multiple_thresholds_notify_independently() {
        let devices = vec![dev("耳机", Some(10))];
        let mut notified = HashSet::new();

        let pending = select_pending_notices(
            &devices,
            true,
            &sel(&["耳机"]),
            &[20, 10],
            &HashMap::new(),
            &mut notified,
        );

        assert_eq!(pending.len(), 2, "两个阈值都应命中: {pending:?}");
        let hits: Vec<i32> = pending.iter().map(|p| p.threshold).collect();
        assert!(hits.contains(&20) && hits.contains(&10));

        // 再跑一轮：两个阈值都已通知过 ⇒ 一条都不弹
        let again = select_pending_notices(
            &devices,
            true,
            &sel(&["耳机"]),
            &[20, 10],
            &HashMap::new(),
            &mut notified,
        );
        assert!(again.is_empty(), "两个阈值都已去过重");
    }

    /// 自定义显示名优先于设备原始名（用户重命名后通知里应显示新名字）。
    ///
    /// 注：本用例同时锁定「每台设备不再各取一次配置锁」的重构——
    /// 名字快照只取一次，与逐台去取在结果上必须完全一致。
    #[test]
    fn custom_display_name_wins_over_raw_name() {
        let devices = vec![dev("WH-1000XM5", Some(5))];
        let mut notified = HashSet::new();

        let pending = select_pending_notices(
            &devices,
            true,
            &sel(&["WH-1000XM5"]),
            &[10],
            &names(&[("WH-1000XM5", "我的耳机")]),
            &mut notified,
        );

        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].display_name, "我的耳机",
            "应使用用户自定义显示名"
        );
    }

    /// 开关关闭 / 阈值列表为空 / 未选择设备 ⇒ 一律不产生通知。
    ///
    /// 三条早退分支各自独立：少写任何一条都会让用户在关闭功能后仍收到提醒，
    /// 或让「未勾选任何设备」变成「所有低电量设备都提醒」这样的大范围打扰。
    #[test]
    fn disabled_or_unconfigured_yields_no_notices() {
        let devices = vec![dev("耳机", Some(1))];
        let chosen = sel(&["耳机"]);

        // 开关关闭
        let mut notified = HashSet::new();
        assert!(select_pending_notices(
            &devices,
            false,
            &chosen,
            &[50],
            &HashMap::new(),
            &mut notified
        )
        .is_empty());

        // 阈值列表为空
        let mut notified = HashSet::new();
        assert!(select_pending_notices(
            &devices,
            true,
            &chosen,
            &[],
            &HashMap::new(),
            &mut notified
        )
        .is_empty());

        // 未选择任何设备
        let mut notified = HashSet::new();
        assert!(
            select_pending_notices(&devices, true, &[], &[50], &HashMap::new(), &mut notified)
                .is_empty()
        );
    }

    /// 未选择的设备、无电量数据的设备都不得命中。
    ///
    /// 前者是用户显式排除；后者若按「无数据视为 0%」处理会制造大量假告警
    /// （读数失败、设备刚连上尚未上报都会是 `None`）。
    #[test]
    fn unselected_or_unknown_level_is_skipped() {
        let devices = vec![
            dev("耳机", Some(1)),
            dev("键盘", None),
            dev("鼠标", Some(2)),
        ];
        let mut notified = HashSet::new();

        let pending: Vec<PendingBatteryNotice> = select_pending_notices(
            &devices,
            true,
            // 只勾选耳机；鼠标虽低电量但未勾选
            &sel(&["耳机", "键盘"]),
            &[50],
            &HashMap::new(),
            &mut notified,
        );

        assert_eq!(pending.len(), 1, "只应有耳机命中: {pending:?}");
        assert_eq!(pending[0].display_name, "耳机");
    }

    // ── P3-3：回差（重新武装）──────────────────────────────────────

    /// 回差判据边界**两侧都断言**：`threshold + 5` 不武装、`threshold + 6` 武装。
    #[test]
    fn rearm_boundary_excludes_the_jitter_band() {
        assert!(!is_rearmed(20, 20), "仍在阈值上，不武装");
        assert!(!is_rearmed(21, 20), "抖动带内，不武装");
        assert!(
            !is_rearmed(25, 20),
            "恰好 = 阈值 + 回差，不武装（边界取严格大于）"
        );
        assert!(is_rearmed(26, 20), "超出回差 1%，武装");
        assert!(is_rearmed(100, 20), "充满，武装");
        assert!(!is_rearmed(19, 20), "低于阈值，不武装");
    }

    /// 核心回归：**放电 → 充满 → 再放电，应重新提醒**。
    /// 修复前 `notified` 只插不删，第二次掉到阈值会被永久静默。
    #[test]
    fn recharge_then_drain_notifies_again() {
        let mut notified = HashSet::new();
        let sel_ear = sel(&["耳机"]);

        // ① 掉到 20%（= 阈值）→ 提醒
        let first = select_pending_notices(
            &[dev("耳机", Some(20))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert_eq!(first.len(), 1, "首次掉到阈值应提醒");

        // ② 同一水位再跑一轮 → 不重复提醒
        let dup = select_pending_notices(
            &[dev("耳机", Some(20))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert!(dup.is_empty(), "同一水位不得重复提醒");

        // ③ 充到 100%（远超阈值 + 回差）→ 标记应被清掉
        let charging = select_pending_notices(
            &[dev("耳机", Some(100))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert!(charging.is_empty(), "充电中不应弹提醒");
        assert!(
            !notified.contains(&("耳机".to_string(), 20)),
            "电量明确回升后，去重标记应被清除"
        );

        // ④ 再次掉到 20% → **必须重新提醒**（这是修复的核心）
        let again = select_pending_notices(
            &[dev("耳机", Some(20))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert_eq!(again.len(), 1, "充满后再放电应重新提醒（修复前此处为空）");
    }

    /// 抖动带内反复横跳**不得**反复弹窗：这正是回差存在的意义。
    #[test]
    fn jitter_within_band_does_not_re_alert() {
        let mut notified = HashSet::new();
        let sel_ear = sel(&["耳机"]);

        // 首次 20% 提醒
        let first = select_pending_notices(
            &[dev("耳机", Some(20))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert_eq!(first.len(), 1);

        // 21~25 之间来回抖（插拔电源最典型的水位）——都在回差带内
        for level in [21, 25, 24, 22, 25, 21] {
            let pending = select_pending_notices(
                &[dev("耳机", Some(level))],
                true,
                &sel_ear,
                &[20],
                &HashMap::new(),
                &mut notified,
            );
            assert!(
                pending.is_empty(),
                "水位 {}% 在回差带内（≤ 阈值 + 5），不得重新提醒",
                level
            );
        }

        // 抖回阈值以下——标记仍在，不该再提醒
        let back_down = select_pending_notices(
            &[dev("耳机", Some(19))],
            true,
            &sel_ear,
            &[20],
            &HashMap::new(),
            &mut notified,
        );
        assert!(back_down.is_empty(), "抖动带内回落后不得再提醒");
    }

    /// 各阈值独立重新武装：清掉一个不该影响另一个。
    #[test]
    fn rearm_is_per_threshold() {
        let mut notified = HashSet::new();
        let sel_ear = sel(&["耳机"]);

        // 10% 时两个阈值都命中
        let first = select_pending_notices(
            &[dev("耳机", Some(10))],
            true,
            &sel_ear,
            &[20, 10],
            &HashMap::new(),
            &mut notified,
        );
        assert_eq!(first.len(), 2);

        // 回升到 16%：> 10 + 5 = 15 ⇒ 10% 这一档重新武装；
        // 但 16 远不够 20 + 5 = 25 ⇒ 20% 那档**仍保持已通知**
        let mid = select_pending_notices(
            &[dev("耳机", Some(16))],
            true,
            &sel_ear,
            &[20, 10],
            &HashMap::new(),
            &mut notified,
        );
        assert!(mid.is_empty(), "16% 高于所有阈值，本轮不弹");
        assert!(
            !notified.contains(&("耳机".to_string(), 10)),
            "10% 档应已重新武装"
        );
        assert!(
            notified.contains(&("耳机".to_string(), 20)),
            "20% 档的回差线是 25%，16% 不足以重新武装它"
        );

        // 掉到 9%：只有 10% 档该重新提醒（20% 档仍未武装）
        let drop = select_pending_notices(
            &[dev("耳机", Some(9))],
            true,
            &sel_ear,
            &[20, 10],
            &HashMap::new(),
            &mut notified,
        );
        assert_eq!(drop.len(), 1, "只应重新提醒 10% 档: {drop:?}");
        assert_eq!(drop[0].threshold, 10);
    }
}

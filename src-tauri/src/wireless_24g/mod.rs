// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 设备电量查看对外入口：驱动注册表查找 + TTL 缓存 + 后台惰性刷新。
// 设备列表只读缓存（即时返回），实际 HID 查询由后台线程完成；
// 日志标签统一为 [24g]。
//
// 缓存语义（stale-while-revalidate）：
// - 成功值永不过期性丢失——TTL 过期后仍返回旧值供 UI 常驻，同时触发刷新，
//   新值到达后经 24g-battery-updated 事件推送前端原地替换；
// - 查询失败不抹除既有成功值，仅推进重试时钟；从未成功过的失败走负缓存。
// - 成功值经 persist 落盘（data/24g_battery_cache.json），进程重启后以
//   「已过期」种子载入——SWR 延伸至跨重启，首启即显上次电量。

mod drivers;
mod hid_link;
mod persist;

// 识别注册表（device_data）消费驱动声明的身份清单；hid_link 等实现细节不外露
pub use drivers::DRIVERS;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use tauri::Emitter;

use hid_link::HidLink;

/// 成功电量的缓存有效期
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// 失败负缓存的有效期（避免对休眠设备反复敲门）
const NEG_TTL: Duration = Duration::from_secs(60);
/// Microsoft VID：Xbox 360 / Xbox One 手柄（XInput 模式）
const MS_VID: u16 = 0x045E;

static CACHE: OnceLock<Mutex<HashMap<(String, String), CacheEntry>>> = OnceLock::new();
/// 后台刷新线程单飞标记（防止多轮列表刷新并发查询）
static REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// 事件推送句柄（main setup 注入）
static EVENT_HANDLE: OnceLock<tauri::AppHandle> = OnceLock::new();

struct CacheEntry {
    /// Some=最后已知电量百分比；None=从未成功过（负缓存）
    level: Option<i32>,
    /// 最近一次尝试时刻（成功与失败均推进，作为刷新/负缓存时钟）
    at: Instant,
    /// 最后一次成功查询的墙钟时间（Unix 秒）；失败不刷新，用于落盘超龄淘汰
    seen: u64,
}

// ── 对外入口 ────────────────────────────────────────────

fn cache() -> &'static Mutex<HashMap<(String, String), CacheEntry>> {
    CACHE.get_or_init(|| {
        // 磁盘种子：成功值跨重启常驻，种子条目置为已过期——首次列表查询
        // 即返回旧值并自动排入后台现查。checked_sub 防开机不足 TTL 时
        // Instant 下溢（autostart 场景）；此时退化为短暂 fresh，最多延迟
        // 一个 TTL 补查，显示不受影响
        let stale_at = Instant::now()
            .checked_sub(CACHE_TTL)
            .unwrap_or_else(Instant::now);
        let map: HashMap<_, _> = persist::load(&persist::cache_path())
            .into_iter()
            .map(|(k, (lv, seen))| {
                (
                    k,
                    CacheEntry {
                        level: Some(lv),
                        at: stale_at,
                        seen,
                    },
                )
            })
            .collect();
        if !map.is_empty() {
            crate::process::append_log(&format!("[24g] 电量缓存自盘载入 {} 条", map.len()));
        }
        Mutex::new(map)
    })
}

/// 解析 4 位十六进制 VID/PID 字符串
fn parse_hex(s: &str) -> Option<u16> {
    u16::from_str_radix(s, 16).ok()
}

/// 驱动是否支持该 VID/PID（vid/pid 为 4 位十六进制大写字符串）
pub fn supported(vid: &str, pid: &str) -> bool {
    match (parse_hex(vid), parse_hex(pid)) {
        (Some(v), Some(p)) => drivers::find_driver(v, p).is_some(),
        _ => false,
    }
}

/// 动态显示名覆盖（罗技单下游：返回下游设备名，如 "MX Master 3S"）；
/// 无覆盖时 None。字符串参数与识别注册表口径一致
pub fn display_override_name(vid: &str, pid: &str) -> Option<String> {
    let (v, p) = parse_hex(vid).zip(parse_hex(pid))?;
    let driver = drivers::find_driver(v, p)?;
    driver.display_override(v, p)
}

/// 注入事件推送句柄（main setup 调用一次）
pub fn init_event_handle(app: &tauri::AppHandle) {
    EVENT_HANDLE.set(app.clone()).ok();
}

/// 电量发生实质变化后通知前端静默重拉（未注入句柄时静默跳过）
fn notify_battery_changed() {
    if let Some(app) = EVENT_HANDLE.get() {
        let _ = app.emit("24g-battery-updated", ());
    }
}

/// 设备列表入口：返回各 (vid,pid) 的缓存电量；过期/缺失项触发后台刷新。
/// stale-while-revalidate：过期成功条目仍返回旧值（UI 常驻），新值经事件推送。
/// force=true 时同步逐台现查（设备列表手动刷新按钮入口，绕过 TTL）。
pub fn snapshot(
    mut pairs: Vec<(String, String)>,
    force: bool,
) -> HashMap<(String, String), Option<i32>> {
    pairs.sort();
    pairs.dedup();

    if force {
        return snapshot_fresh(pairs);
    }

    let now = Instant::now();
    let mut result = HashMap::new();
    let mut stale = vec![];

    {
        let guard = crate::state::lock_unpoisoned(cache());
        for key in &pairs {
            let k = format!("{}:{}", key.0, key.1);
            match guard.get(key) {
                Some(e) => {
                    let fresh = now.duration_since(e.at) < ttl_of(e);
                    // 过期（成功或失败）都排入后台刷新队列
                    if !fresh && !stale.contains(key) {
                        stale.push(key.clone());
                    }
                    // 成功过的条目常驻旧值；纯失败态仅在负缓存窗口内返回 None
                    if fresh || e.level.is_some() {
                        if !fresh {
                            crate::process::append_verbose_log(&format!(
                                "[24g:dbg] {} 过期，SWR 服务旧值并排入刷新",
                                k
                            ));
                        }
                        result.insert(key.clone(), e.level);
                    } else {
                        crate::process::append_verbose_log(&format!(
                            "[24g:dbg] {} 负缓存窗口内，返回无数据",
                            k
                        ));
                        result.insert(key.clone(), None);
                    }
                }
                None => {
                    crate::process::append_verbose_log(&format!(
                        "[24g:dbg] {} 无缓存条目（冷启动），排入刷新",
                        k
                    ));
                    if !stale.contains(key) {
                        stale.push(key.clone());
                    }
                    result.insert(key.clone(), None);
                }
            }
        }
    }

    // 单飞触发后台刷新：已有线程在跑则跳过本轮，待其结束后下轮补查
    if !stale.is_empty() {
        if REFRESHING
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
        {
            crate::process::append_log(&format!(
                "[24g] 后台刷新开始: {} 台（来源：惰性补查）",
                stale.len()
            ));
            std::thread::spawn(move || {
                let started = std::time::Instant::now();
                refresh_worker(stale);
                crate::process::append_log(&format!(
                    "[24g] 后台刷新耗时 {}ms",
                    started.elapsed().as_millis()
                ));
                REFRESHING.store(false, std::sync::atomic::Ordering::SeqCst);
            });
        } else {
            crate::process::append_log(&format!(
                "[24g] 已有后台刷新进行中，跳过本轮（{} 台待查）",
                stale.len()
            ));
        }
    }
    result
}

// ── 缓存与刷新 ───────────────────────────────────────────

fn ttl_of(entry: &CacheEntry) -> Duration {
    if entry.level.is_some() {
        CACHE_TTL
    } else {
        NEG_TTL
    }
}

/// 合并规则：成功更新值；失败保留既有成功值（仅推进重试时钟）。
/// 返回新条目与「是否发生实质变化」（None↔有值、数值变动），供条件推送判定。
fn apply_result(old: Option<&CacheEntry>, result: &Result<i32, String>) -> (CacheEntry, bool) {
    let level = match result {
        Ok(lv) => Some(*lv),
        Err(_) => old.and_then(|e| e.level),
    };
    let old_level = old.and_then(|e| e.level);
    let changed = old_level != level;
    // 仅成功查询刷新 seen；失败保留旧值，使持续离线的设备在超龄后自然淘汰
    let seen = match result {
        Ok(_) => persist::now_unix(),
        Err(_) => old.map(|e| e.seen).unwrap_or(0),
    };
    (
        CacheEntry {
            level,
            at: Instant::now(),
            seen,
        },
        changed,
    )
}

/// 单台查询结果（调用方分别用于事件推送判定与批量收尾落盘）
struct QueryOutcome {
    /// 写回缓存后的电量值
    level: Option<i32>,
    /// 相对旧缓存是否实质变化（None↔有值、数值变动），驱动条件推送
    changed: bool,
    /// 本次是否现查到成功值，驱动批量收尾落盘
    queried_ok: bool,
}

/// 查询单台设备并写回缓存
fn query_and_cache(link: Option<&HidLink>, key: &(String, String)) -> QueryOutcome {
    let invalid = QueryOutcome {
        level: None,
        changed: false,
        queried_ok: false,
    };
    let Some((v, p)) = parse_hex(&key.0).zip(parse_hex(&key.1)) else {
        return invalid;
    };

    // XInput 设备走独立路径（XInputDriver 的 read_battery 是空桩）
    if v == MS_VID {
        let result = match crate::xinput::scan_battery() {
            Some(pct) => Ok(pct),
            None => Err("XInput 无可用控制器".to_string()),
        };
        match &result {
            Ok(lv) => crate::process::append_log(&format!(
                "[24g] XInput {:04X}:{:04X} 电量 {}%",
                v, p, lv
            )),
            Err(e) => crate::process::append_verbose_log(&format!(
                "[24g:dbg] XInput {:04X}:{:04X} 查询失败: {}",
                v, p, e
            )),
        }
        let mut guard = crate::state::lock_unpoisoned(cache());
        let (entry, changed) = apply_result(guard.get(key), &result);
        let outcome = QueryOutcome {
            level: entry.level,
            changed,
            queried_ok: result.is_ok(),
        };
        guard.insert(key.clone(), entry);
        return outcome;
    }

    // 标准 HID 驱动查找（link 为空表示 HID 会话初始化失败，按负缓存处理）
    let Some(link) = link else {
        return invalid;
    };
    if let Some(driver) = drivers::find_driver(v, p) {
        let label = match driver.device_name(v, p) {
            Some(name) => format!("{} ({:04X}:{:04X})", name, v, p),
            None => format!("{:04X}:{:04X}", v, p),
        };
        let result = driver.read_battery(link, v, p);
        match &result {
            Ok(lv) => crate::process::append_log(&format!("[24g] {} 电量 {}%", label, lv)),
            Err(e) => crate::process::append_log(&format!("[24g] {} 查询失败: {}", label, e)),
        }
        let mut guard = crate::state::lock_unpoisoned(cache());
        let (entry, changed) = apply_result(guard.get(key), &result);
        let outcome = QueryOutcome {
            level: entry.level,
            changed,
            queried_ok: result.is_ok(),
        };
        guard.insert(key.clone(), entry);
        return outcome;
    }

    invalid
}

/// 强制刷新路径（手动刷新按钮）：在调用方阻塞线程中同步逐台现查并返回最新值。
/// 后台刷新线程恰好在跑时退化为读缓存，避免并发访问同一 HID 设备。
fn snapshot_fresh(pairs: Vec<(String, String)>) -> HashMap<(String, String), Option<i32>> {
    if REFRESHING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        let guard = crate::state::lock_unpoisoned(cache());
        return pairs
            .into_iter()
            .map(|k| {
                let lvl = guard.get(&k).and_then(|e| e.level);
                (k, lvl)
            })
            .collect();
    }

    crate::process::append_log(&format!("[24g] 强制刷新开始: {} 台", pairs.len()));
    let started = std::time::Instant::now();
    let link = HidLink::new().ok();
    let mut result = HashMap::new();
    let (mut ok, mut fail) = (0, 0);
    let mut any_changed = false;
    let mut any_queried_ok = false;
    for key in &pairs {
        let o = query_and_cache(link.as_ref(), key);
        match o.level {
            Some(_) => ok += 1,
            None => fail += 1,
        }
        any_changed |= o.changed;
        any_queried_ok |= o.queried_ok;
        result.insert(key.clone(), o.level);
    }
    if any_changed {
        notify_battery_changed();
    }
    // 本轮查到过成功值才落盘（degraded 读缓存路径无写入，不 flush）
    if any_queried_ok {
        persist::flush();
    }
    REFRESHING.store(false, std::sync::atomic::Ordering::SeqCst);
    crate::process::append_log(&format!(
        "[24g] 强制刷新结束(耗时 {}ms): 成功 {} 失败 {}",
        started.elapsed().as_millis(),
        ok,
        fail
    ));
    result
}

/// 后台线程体：逐台查询并写回缓存（成功与失败均记录，便于诊断休眠/离线）；
/// 本轮存在实质变化时推送前端，查到过成功值时收尾落盘一次
fn refresh_worker(pairs: Vec<(String, String)>) {
    let link = HidLink::new().ok();
    let (mut ok, mut fail) = (0, 0);
    let mut any_changed = false;
    let mut any_queried_ok = false;
    for key in &pairs {
        let o = query_and_cache(link.as_ref(), key);
        match o.level {
            Some(_) => ok += 1,
            None => fail += 1,
        }
        any_changed |= o.changed;
        any_queried_ok |= o.queried_ok;
    }
    crate::process::append_log(&format!("[24g] 后台刷新结束: 成功 {} 失败 {}", ok, fail));
    if any_changed {
        notify_battery_changed();
        crate::process::append_log("[24g] 已推送电量变更事件");
    }
    if any_queried_ok {
        persist::flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(level: Option<i32>) -> CacheEntry {
        CacheEntry {
            level,
            at: Instant::now(),
            seen: persist::now_unix(),
        }
    }

    #[test]
    fn apply_result_success_updates_value() {
        let (e, changed) = apply_result(Some(&entry(Some(9))), &Ok(12));
        assert_eq!(e.level, Some(12));
        assert!(changed, "数值变动应判定为实质变化");
    }

    #[test]
    fn apply_result_failure_preserves_known_value() {
        // 失败不得抹除既有成功值（SWR 常驻语义）
        let (e, changed) = apply_result(Some(&entry(Some(9))), &Err("超时".into()));
        assert_eq!(e.level, Some(9));
        assert!(!changed);
    }

    #[test]
    fn apply_result_first_failure_enters_negative_cache() {
        let (e, changed) = apply_result(None, &Err("离线".into()));
        assert_eq!(e.level, None);
        assert!(!changed);
    }

    #[test]
    fn apply_result_success_refreshes_seen() {
        // 成功查询推进 seen，保证持续在线的设备不被超龄淘汰
        let old = entry(Some(9));
        let (e, _) = apply_result(Some(&old), &Ok(12));
        assert!(e.seen >= old.seen, "成功查询应刷新 last_seen");
    }

    #[test]
    fn apply_result_failure_preserves_seen() {
        // 失败保留旧 seen，使持续离线的设备在超龄后能被自然淘汰
        let old = entry(Some(9));
        let (e, _) = apply_result(Some(&old), &Err("离线".into()));
        assert_eq!(e.seen, old.seen, "失败查询不应刷新 last_seen");
    }
}

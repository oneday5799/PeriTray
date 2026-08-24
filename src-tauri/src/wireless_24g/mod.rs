// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 设备电量查看对外入口：驱动注册表查找 + TTL 缓存 + 后台惰性刷新。
// 设备列表只读缓存（即时返回），实际 HID 查询由后台线程完成；
// 日志标签统一为 [24g]。

mod drivers;
mod hid_link;

// 识别注册表（device_data）消费驱动声明的身份清单；hid_link 等实现细节不外露
pub use drivers::DRIVERS;

use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// 成功电量的缓存有效期
const CACHE_TTL: Duration = Duration::from_secs(5 * 60);
/// 失败负缓存的有效期（避免对休眠设备反复敲门）
const NEG_TTL: Duration = Duration::from_secs(60);

static CACHE: OnceLock<Mutex<HashMap<(String, String), CacheEntry>>> = OnceLock::new();
/// 后台刷新线程单飞标记（防止多轮列表刷新并发查询）
static REFRESHING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

struct CacheEntry {
    /// Some=电量百分比；None=近期查询失败（负缓存）
    level: Option<i32>,
    at: Instant,
}

// ── 对外入口 ────────────────────────────────────────────

fn cache() -> &'static Mutex<HashMap<(String, String), CacheEntry>> {
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
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

/// 设备列表入口：返回各 (vid,pid) 的缓存电量；过期/缺失项触发后台刷新，
/// 结果下次列表刷新可见。返回映射仅含传入的键，值 None 表示暂无有效数据。
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
            match guard.get(key) {
                Some(e) if now.duration_since(e.at) < ttl_of(e) => {
                    result.insert(key.clone(), e.level);
                }
                _ => {
                    if !stale.contains(key) {
                        stale.push(key.clone());
                    }
                    result.insert(key.clone(), None);
                }
            }
        }
    }

    // 单飞触发后台刷新：已有线程在跑则跳过本轮，待其结束后下轮补查
    if !stale.is_empty()
        && REFRESHING
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
            )
            .is_ok()
    {
        std::thread::spawn(move || {
            refresh_worker(stale);
            REFRESHING.store(false, std::sync::atomic::Ordering::SeqCst);
        });
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

/// 查询单台设备并写回缓存，返回电量值（无驱动支持时返回 None）
fn query_and_cache(key: &(String, String)) -> Option<i32> {
    let Some((v, p)) = parse_hex(&key.0).zip(parse_hex(&key.1)) else {
        return None;
    };
    let Some(driver) = drivers::find_driver(v, p) else {
        return None;
    };
    // 日志优先带设备名，便于社区反馈定位
    let label = match driver.device_name(v, p) {
        Some(name) => format!("{} ({:04X}:{:04X})", name, v, p),
        None => format!("{:04X}:{:04X}", v, p),
    };
    let level = driver.read_battery(v, p);
    match &level {
        Ok(lv) => crate::process::append_log(&format!("[24g] {} 电量 {}%", label, lv)),
        Err(e) => crate::process::append_log(&format!("[24g] {} 查询失败: {}", label, e)),
    }
    let level = level.ok();
    crate::state::lock_unpoisoned(cache()).insert(
        key.clone(),
        CacheEntry {
            level,
            at: Instant::now(),
        },
    );
    level
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

    let mut result = HashMap::new();
    for key in &pairs {
        let level = query_and_cache(key);
        result.insert(key.clone(), level);
    }
    REFRESHING.store(false, std::sync::atomic::Ordering::SeqCst);
    result
}

/// 后台线程体：逐台查询并写回缓存（成功与失败均记录，便于诊断休眠/离线）
fn refresh_worker(pairs: Vec<(String, String)>) {
    for key in &pairs {
        query_and_cache(key);
    }
}

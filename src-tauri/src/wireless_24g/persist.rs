// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 成功电量缓存跨重启持久化：data/24g_battery_cache.json。
// 仅持久化成功值（失败/负缓存不落盘），扁平 "VID:PID" → level；
// 损坏文件静默降级为空表 + 日志，下次成功覆写自愈。

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// 成功电量缓存的最大保留时长：超过该时长未再成功查询的条目在加载时淘汰。
const MAX_AGE_SECS: u64 = 30 * 24 * 3600;

/// 当前墙钟 Unix 秒（超龄淘汰与 seeen 时间戳共用）。
pub(crate) fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

/// 新格式落盘条目：level + 最后成功查询时间。
#[derive(Serialize, Deserialize)]
struct Entry {
    level: i32,
    #[serde(default)]
    seen: u64,
}

/// 容忍历史旧格式（裸整数电量）的取值：旧值按当前时间补 seen，保留一周期后重写。
#[derive(Deserialize)]
#[serde(untagged)]
enum RawValue {
    Level(i32),
    Entry(Entry),
}

pub(crate) fn cache_path() -> std::path::PathBuf {
    crate::process::data_dir().join("24g_battery_cache.json")
}

/// 读盘还原 (vid,pid) → (电量, last_seen)；键非法条目跳过，缺失/损坏返回空表，
/// 超龄（MAX_AGE_SECS 内未再成功查询）条目淘汰。
pub(crate) fn load(path: &Path) -> HashMap<(String, String), (i32, u64)> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        // 文件不存在属正常态（首次运行或从未查到过电量）
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str::<HashMap<String, RawValue>>(&content) {
        Ok(raw) => {
            let now = now_unix();
            let mut result = HashMap::new();
            for (key, value) in raw {
                let Some(pair) = parse_key(&key) else {
                    continue;
                };
                let (level, seen) = match value {
                    // 旧格式裸整数：无时间戳，按当前时间补 seen（保留一周期）
                    RawValue::Level(lv) => (lv, now),
                    // 新格式：seen==0 视为旧值，同样按当前时间补齐
                    RawValue::Entry(Entry { level, seen }) if seen == 0 => (level, now),
                    RawValue::Entry(Entry { level, seen }) => (level, seen),
                };
                if now.saturating_sub(seen) > MAX_AGE_SECS {
                    crate::process::append_log(&format!("[24g] 淘汰超龄电量缓存条目 {}", key));
                    continue;
                }
                result.insert(pair, (level, seen));
            }
            result
        }
        Err(e) => {
            crate::process::append_log(&format!(
                "[24g] 电量缓存损坏，忽略重建 ({}): {}",
                path.display(),
                e
            ));
            HashMap::new()
        }
    }
}

/// "VID:PID" → ("VID","PID")；无分隔符或分段为空视为非法
fn parse_key(key: &str) -> Option<(String, String)> {
    let (vid, pid) = key.split_once(':')?;
    if vid.is_empty() || pid.is_empty() {
        return None;
    }
    Some((vid.to_string(), pid.to_string()))
}

/// 内存缓存中的全部成功值快照（锁内收集，调用方在锁外落盘），含 last_seen 时间戳
fn collect_successes() -> HashMap<(String, String), (i32, u64)> {
    crate::state::lock_unpoisoned(super::cache())
        .iter()
        .filter_map(|(k, e)| e.level.map(|lv| (k.clone(), (lv, e.seen))))
        .collect()
}

/// 收集成功值并写盘（一批查询完成后调用一次；失败仅记日志）
pub(crate) fn flush() {
    let successes = collect_successes();
    if successes.is_empty() {
        return;
    }
    if let Err(e) = save(&successes, &cache_path()) {
        crate::process::append_log(&format!("[24g] 电量缓存写盘失败: {}", e));
    }
}

fn save(map: &HashMap<(String, String), (i32, u64)>, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let raw: HashMap<String, Entry> = map
        .iter()
        .map(|((v, p), (lv, seen))| {
            (
                format!("{}:{}", v, p),
                Entry {
                    level: *lv,
                    seen: *seen,
                },
            )
        })
        .collect();
    std::fs::write(path, serde_json::to_string_pretty(&raw).unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp_path(tag: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "pm_24g_persist_{}_{}.json",
            tag,
            std::process::id()
        ))
    }

    fn sample() -> HashMap<(String, String), (i32, u64)> {
        let now = now_unix();
        HashMap::from([
            (("1532".into(), "0094".into()), (85, now)),
            (("046D".into(), "C52B".into()), (37, now)),
        ])
    }

    #[test]
    fn round_trip_preserves_keys_and_values() {
        let path = tmp_path("roundtrip");
        save(&sample(), &path).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&("1532".to_string(), "0094".to_string())].0, 85);
        assert_eq!(loaded[&("046D".to_string(), "C52B".to_string())].0, 37);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_yields_empty() {
        let path = tmp_path("missing_nonexistent_dir");
        assert!(load(&path).is_empty());
    }

    #[test]
    fn corrupt_json_yields_empty() {
        let path = tmp_path("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "{not json").unwrap();
        assert!(load(&path).is_empty());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn invalid_keys_are_skipped() {
        let path = tmp_path("badkeys");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"NOSEP": 50, ":": 60, "046D:C52B": 42}"#).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&("046D".to_string(), "C52B".to_string())].0, 42);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn legacy_int_format_is_accepted() {
        let path = tmp_path("legacy");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"046D:C52B": 42}"#).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[&("046D".to_string(), "C52B".to_string())].0, 42);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn aged_entry_is_evicted() {
        let path = tmp_path("aged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = now_unix() - (MAX_AGE_SECS + 3600);
        let json = format!(r#"{{"046D:C52B":{{"level":42,"seen":{stale}}}}}"#);
        std::fs::write(&path, json).unwrap();
        let loaded = load(&path);
        assert!(loaded.is_empty(), "超龄条目应在加载时被淘汰");
        std::fs::remove_file(&path).ok();
    }
}

// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 成功电量缓存跨重启持久化：data/24g_battery_cache.json。
// 仅持久化成功值（失败/负缓存不落盘），扁平 <设备身份键> → level；
// 损坏文件静默降级为空表 + 日志，下次成功覆写自愈。
//
// ⚠️ 键是**设备身份键**（`c:` 容器 / `i:` 实例 / `n:` 名称 / `m:` 型号兜底，
// 见 `device_identity::DeviceKey`），**不是 `VID:PID`**。
// 历史版本用 `VID:PID` 作键，那是**型号**级标识 ⇒ 两个同款 2.4G 接收器
// 共用一条缓存、只有一台能显示电量，且显示的值可能来自另一台。
// 旧键**无法**归属到具体设备，故加载时一律丢弃（缓存可重建，丢一轮不伤数据）。

use std::collections::HashMap;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::standard_log;
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

/// 合法设备身份键：`c:` 容器 / `i:` 实例 / `n:` 名称 / `m:` 型号兜底，
/// 且载荷非空。
///
/// 之所以要显式校验：历史文件的键是 `VID:PID` 形态，它也含 `:`，
/// 若不校验就会被当成合法键加载进来 —— 那是**型号**级键，
/// 会让同款设备重新串号（正是本次要修掉的缺陷）。
fn is_device_key(key: &str) -> bool {
    matches!(
        key.get(..2),
        Some("c:") | Some("i:") | Some("n:") | Some("m:")
    ) && key.len() > 2
}

/// 读盘还原 <设备身份键> → (电量, last_seen)；键非法条目跳过，缺失/损坏返回空表，
/// 超龄（MAX_AGE_SECS 内未再成功查询）条目淘汰。
pub(crate) fn load(path: &Path) -> HashMap<String, (i32, u64)> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        // 文件不存在属正常态（首次运行或从未查到过电量）
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str::<HashMap<String, RawValue>>(&content) {
        Ok(raw) => {
            let now = now_unix();
            let mut result = HashMap::new();
            let mut legacy = 0usize;
            for (key, value) in raw {
                if !is_device_key(&key) {
                    legacy += 1;
                    continue;
                }
                let (level, seen) = match value {
                    // 旧格式裸整数：无时间戳，按当前时间补 seen（保留一周期）
                    RawValue::Level(lv) => (lv, now),
                    // 新格式：seen==0 视为旧值，同样按当前时间补齐
                    RawValue::Entry(Entry { level, seen: 0 }) => (level, now),
                    RawValue::Entry(Entry { level, seen }) => (level, seen),
                };
                if now.saturating_sub(seen) > MAX_AGE_SECS {
                    standard_log!("[24g] 淘汰超龄电量缓存条目 {}", key);
                    continue;
                }
                result.insert(key, (level, seen));
            }
            if legacy > 0 {
                standard_log!(
                    "[24g] 丢弃 {} 条旧「VID:PID」键的电量缓存（型号级键，无法归属到具体设备）",
                    legacy
                );
            }
            result
        }
        Err(e) => {
            standard_log!("[24g] 电量缓存损坏，忽略重建 ({}): {}", path.display(), e);
            HashMap::new()
        }
    }
}

/// 内存缓存中的全部成功值快照（锁内收集，调用方在锁外落盘），含 last_seen 时间戳
fn collect_successes() -> HashMap<String, (i32, u64)> {
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
        standard_log!("[24g] 电量缓存写盘失败: {}", e);
    }
}

fn save(map: &HashMap<String, (i32, u64)>, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let raw: HashMap<&String, Entry> = map
        .iter()
        .map(|(k, (lv, seen))| {
            (
                k,
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

    fn sample() -> HashMap<String, (i32, u64)> {
        let now = now_unix();
        HashMap::from([
            (
                "c:40e11c06-72bd-5b38-9bd2-0e15079b3b45".to_string(),
                (85, now),
            ),
            (
                "i:usb\\vid_046d&pid_c52b\\5&1a2b3c4d&0&1".to_string(),
                (37, now),
            ),
        ])
    }

    #[test]
    fn round_trip_preserves_keys_and_values() {
        let path = tmp_path("roundtrip");
        save(&sample(), &path).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded["c:40e11c06-72bd-5b38-9bd2-0e15079b3b45"].0, 85);
        assert_eq!(loaded["i:usb\\vid_046d&pid_c52b\\5&1a2b3c4d&0&1"].0, 37);
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

    /// 核心判据：历史 `VID:PID` 键**必须被丢弃**。
    ///
    /// 它们是**型号**级键 ⇒ 若被当作合法键载入，两个同款接收器会重新共用一条缓存，
    /// 本次要修的串号缺陷就会「复活」。可证伪：把 `is_device_key` 改成恒真即转红。
    #[test]
    fn legacy_vid_pid_keys_are_dropped() {
        let path = tmp_path("legacyvidpid");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"046D:C52B": 42, "1532:0094": 77, "c:aaaa": 55}"#).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1, "只应保留设备身份键那条：{loaded:?}");
        assert_eq!(loaded["c:aaaa"].0, 55);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn malformed_device_keys_are_skipped() {
        let path = tmp_path("badkeys");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        // 前缀不认识 / 载荷为空 / 纯裸串，都不得进入结果
        std::fs::write(
            &path,
            r#"{"x:abc": 50, "c:": 60, "NOSEP": 70, "m:046D:C52B": 42}"#,
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1, "{loaded:?}");
        assert_eq!(loaded["m:046D:C52B"].0, 42);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn legacy_int_format_is_accepted() {
        let path = tmp_path("legacyint");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, r#"{"c:aaaa": 42}"#).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded["c:aaaa"].0, 42);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn aged_entry_is_evicted() {
        let path = tmp_path("aged");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let stale = now_unix() - (MAX_AGE_SECS + 3600);
        let json = format!(r#"{{"c:aaaa":{{"level":42,"seen":{stale}}}}}"#);
        std::fs::write(&path, json).unwrap();
        let loaded = load(&path);
        assert!(loaded.is_empty(), "超龄条目应在加载时被淘汰");
        std::fs::remove_file(&path).ok();
    }
}

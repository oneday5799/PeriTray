// ── 模块职责 ─────────────────────────────────────────────
// 2.4G 成功电量缓存跨重启持久化：data/24g_battery_cache.json。
// 仅持久化成功值（失败/负缓存不落盘），扁平 "VID:PID" → level；
// 损坏文件静默降级为空表 + 日志，下次成功覆写自愈。

use std::collections::HashMap;
use std::path::Path;

pub(crate) fn cache_path() -> std::path::PathBuf {
    crate::process::exe_dir()
        .join("data")
        .join("24g_battery_cache.json")
}

/// 读盘还原 (vid,pid) → 电量；键非法条目跳过，缺失/损坏返回空表
pub(crate) fn load(path: &Path) -> HashMap<(String, String), i32> {
    let content = match std::fs::read_to_string(path) {
        Ok(c) => c,
        // 文件不存在属正常态（首次运行或从未查到过电量）
        Err(_) => return HashMap::new(),
    };
    match serde_json::from_str::<HashMap<String, i32>>(&content) {
        Ok(raw) => {
            let mut result = HashMap::new();
            for (key, level) in raw {
                if let Some(pair) = parse_key(&key) {
                    result.insert(pair, level);
                }
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

/// 内存缓存中的全部成功值快照（锁内收集，调用方在锁外落盘）
fn collect_successes() -> HashMap<(String, String), i32> {
    crate::state::lock_unpoisoned(super::cache())
        .iter()
        .filter_map(|(k, e)| e.level.map(|lv| (k.clone(), lv)))
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

fn save(map: &HashMap<(String, String), i32>, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let raw: HashMap<String, i32> = map
        .iter()
        .map(|((v, p), lv)| (format!("{}:{}", v, p), *lv))
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

    fn sample() -> HashMap<(String, String), i32> {
        HashMap::from([
            (("1532".into(), "0094".into()), 85),
            (("046D".into(), "C52B".into()), 37),
        ])
    }

    #[test]
    fn round_trip_preserves_keys_and_values() {
        let path = tmp_path("roundtrip");
        save(&sample(), &path).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[&("1532".to_string(), "0094".to_string())], 85);
        assert_eq!(loaded[&("046D".to_string(), "C52B".to_string())], 37);
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
        assert_eq!(loaded[&("046D".to_string(), "C52B".to_string())], 42);
        std::fs::remove_file(&path).ok();
    }
}

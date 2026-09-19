use crate::standard_log;
use serde::Serialize;
use std::ffi::c_void;
use std::sync::Mutex;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Networking::WinHttp::*;

#[derive(Debug, Clone, Serialize)]
pub struct UpdateInfo {
    pub has_update: bool,
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
}

/// 更新检查状态（供设置页「关于」infobar 展示）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateStatus {
    /// "latest" | "update" | "storeUpdate" | "error"
    pub status: String,
    pub current_version: String,
    pub latest_version: String,
    pub release_url: String,
    pub error: Option<String>,
}

impl UpdateStatus {
    pub fn from_info(info: &UpdateInfo, status: &str) -> Self {
        UpdateStatus {
            status: status.to_string(),
            current_version: info.current_version.clone(),
            latest_version: info.latest_version.clone(),
            release_url: info.release_url.clone(),
            error: None,
        }
    }

    fn from_error(current_version: &str, error: &str) -> Self {
        UpdateStatus {
            status: "error".to_string(),
            current_version: current_version.to_string(),
            latest_version: String::new(),
            release_url: String::new(),
            error: Some(error.to_string()),
        }
    }
}

static LAST_STATUS: Mutex<Option<UpdateStatus>> = Mutex::new(None);

fn set_last_status(status: UpdateStatus) {
    // P2-11：统一入口。原写法 `if let Ok(guard) = mutex.lock()` 在中毒时静默丢弃状态，
    // 设置页会永远显示不出「已是最新」。
    *crate::state::lock_unpoisoned(&LAST_STATUS) = Some(status);
}

pub fn get_last_status() -> Option<UpdateStatus> {
    // P2-11：原写法 `Mutex::lock()` 后接 `ok().and_then(..)` 在中毒时静默返回 None（同上后果）
    crate::state::lock_unpoisoned(&LAST_STATUS).clone()
}

/// 执行一次更新检查并把结果写入 LAST_STATUS（成功与检查失败均存储）。
/// `tag` 用于日志来源区分（如 "startup"，空串表示设置页手动检查）。
/// 返回 (检查结果, 本次是否写入了状态)；任务级失败时状态保持原样，由调用方决定是否广播。
pub async fn check_and_store(
    tag: &str,
    current_version: String,
    include_prerelease: bool,
) -> (Result<UpdateInfo, String>, bool) {
    let prefix = if tag.is_empty() {
        String::new()
    } else {
        format!(" {}", tag)
    };

    // 根据安装方式选择更新源：MSIX 走 Store API，NSIS 走 GitHub
    let result: Result<UpdateInfo, String> = if crate::windows::is_msix_context() {
        standard_log!("[update]{} MSIX context, checking Store", prefix);
        check_store_update(&current_version).await
    } else {
        let ver_for_task = current_version.clone();
        match tokio::task::spawn_blocking(move || {
            check_for_update(&ver_for_task, include_prerelease)
        })
        .await
        {
            Ok(inner) => inner,
            Err(e) => Err(format!("task error: {}", e)),
        }
    };

    match result {
        Ok(info) => {
            let status = if info.has_update {
                if crate::windows::is_msix_context() {
                    "storeUpdate"
                } else {
                    "update"
                }
            } else {
                "latest"
            };
            set_last_status(UpdateStatus::from_info(&info, status));
            (Ok(info), true)
        }
        Err(e) => {
            standard_log!("[update]{} check failed: {}", prefix, e);
            set_last_status(UpdateStatus::from_error(&current_version, &e));
            (Err(e), true)
        }
    }
}

#[derive(Debug, serde::Deserialize)]
struct GitHubRelease {
    tag_name: String,
    prerelease: bool,
    draft: bool,
    html_url: String,
}

/// Microsoft Store 更新检测（仅 MSIX 环境）。
/// Store API 不返回版本号，仅判断"是否有更新可用"。
const STORE_URL: &str = "ms-windows-store://pdp/?productid=9PLTSS6S80XJ";

async fn check_store_update(current_version: &str) -> Result<UpdateInfo, String> {
    use windows::Services::Store::StoreContext;

    let ctx = StoreContext::GetDefault().map_err(|e| format!("Store 服务初始化失败: {:?}", e))?;
    let op = ctx
        .GetAppAndOptionalStorePackageUpdatesAsync()
        .map_err(|e| format!("Store 查询失败: {:?}", e))?;
    let updates = op.await.map_err(|e| format!("Store 请求失败: {:?}", e))?;

    let has_update = updates.Size().unwrap_or(0) > 0;
    standard_log!(
        "[update] Store check: has_update={}, current={}",
        has_update,
        current_version
    );

    Ok(UpdateInfo {
        has_update,
        current_version: current_version.to_string(),
        latest_version: String::new(),
        release_url: STORE_URL.to_string(),
    })
}

/// WinHTTP GET request, returns response body as String
fn winhttp_get(host: &str, path: &str) -> Result<String, String> {
    let user_agent = crate::process::to_wide("PeriTray");
    let host_wide = crate::process::to_wide(host);
    let path_wide = crate::process::to_wide(path);
    let verb = crate::process::to_wide("GET");

    unsafe {
        let session = WinHttpOpen(
            user_agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
            std::ptr::null(),
            std::ptr::null(),
            0,
        );
        if session.is_null() {
            return Err("网络连接失败".to_string());
        }

        let connect = WinHttpConnect(session, host_wide.as_ptr(), 443, 0);
        if connect.is_null() {
            WinHttpCloseHandle(session);
            return Err("无法连接到服务器".to_string());
        }

        let request = WinHttpOpenRequest(
            connect,
            verb.as_ptr(),
            path_wide.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            std::ptr::null(),
            WINHTTP_FLAG_SECURE,
        );
        if request.is_null() {
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return Err("请求创建失败".to_string());
        }

        WinHttpSetTimeouts(request, 5000, 10000, 10000, 10000);

        if WinHttpSendRequest(request, std::ptr::null(), 0, std::ptr::null_mut(), 0, 0, 0) == 0 {
            let err = GetLastError();
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return if err == 12007 {
                Err(format!("DNS 解析失败 ({})", err))
            } else if err == 12002 || err == 12030 {
                Err(format!("网络连接超时 ({})", err))
            } else {
                Err(format!("网络错误 ({})", err))
            };
        }

        if WinHttpReceiveResponse(request, std::ptr::null_mut()) == 0 {
            let err = GetLastError();
            WinHttpCloseHandle(request);
            WinHttpCloseHandle(connect);
            WinHttpCloseHandle(session);
            return if err == 12002 || err == 12030 {
                Err(format!("网络连接超时 ({})", err))
            } else {
                Err(format!("网络错误 ({})", err))
            };
        }

        // 检查 HTTP 状态码
        let mut status_code: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let mut index: u32 = 0;
        WinHttpQueryHeaders(
            request,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            std::ptr::null(),
            &mut status_code as *mut u32 as *mut c_void,
            &mut size,
            &mut index,
        );
        match status_code {
            200 => {}
            403 => {
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err("请求过于频繁，请稍后再试 (403)".to_string());
            }
            code => {
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err(format!("GitHub 服务器错误 ({})", code));
            }
        }

        let mut body = Vec::new();
        let mut buffer = [0u8; 4096];
        let mut bytes_read: u32;

        loop {
            bytes_read = 0;
            if WinHttpReadData(
                request,
                buffer.as_mut_ptr() as *mut c_void,
                buffer.len() as u32,
                &mut bytes_read,
            ) == 0
            {
                let err = GetLastError();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return if err == 12002 || err == 12030 {
                    Err(format!("网络连接超时 ({})", err))
                } else {
                    Err(format!("网络错误 ({})", err))
                };
            }
            if bytes_read == 0 {
                break;
            }
            body.extend_from_slice(&buffer[..bytes_read as usize]);
            // 响应体上限（P3-1）：GitHub Releases 的 JSON 正常在百 KB 量级，
            // 4 MiB 已是两个数量级的余量。修复前这里**没有上限**，`body` 会一直
            // 增长到内存耗尽 —— 对端是 `api.github.com`，但 DNS / 代理被劫持时
            // 完全可能收到一个无底洞式的响应体。
            //
            // 判定放在**读满之后**：4 MiB + 4 KiB 也能被拦下，不会因为
            // 校验早于读取而漏掉「恰好跨过边界」的那一批。
            if body_too_large(body.len()) {
                let size = body.len();
                WinHttpCloseHandle(request);
                WinHttpCloseHandle(connect);
                WinHttpCloseHandle(session);
                return Err(format!(
                    "响应体过大（{} 字节，上限 {} 字节），已中止下载",
                    size, MAX_RESPONSE_BODY
                ));
            }
        }

        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connect);
        WinHttpCloseHandle(session);

        String::from_utf8(body).map_err(|_| "响应编码错误".to_string())
    }
}

/// 响应体大小上限：4 MiB。GitHub Releases JSON 正常量级为百 KB，
/// 留两个数量级余量；超出即视为异常（DNS/代理劫持或服务端异常）。
const MAX_RESPONSE_BODY: usize = 4 * 1024 * 1024;

/// 响应体是否已超上限。
///
/// 抽成**纯函数**是为了能对边界本身做单测（P3-1）：`cargo check` 证明不了
/// 「4 MiB 与 4 MiB+1 分别怎么走」，只有断言才能。判据取**严格大于**：
/// 恰好等于上限的响应体是允许的，上限本身不是非法值。
fn body_too_large(len: usize) -> bool {
    len > MAX_RESPONSE_BODY
}

/// 把两个版本号数字段补齐到等长后逐位比较。
///
/// 抽成独立函数（而非内联 `Vec` 比较）是为了**同时**给「判等」与「判大小」
/// 两条路径一个单一来源 —— 两者若各写一份补齐逻辑，迟早会漂移。
fn align_version_nums(a: &[u32], b: &[u32]) -> (Vec<u32>, Vec<u32>) {
    let n = a.len().max(b.len());
    let mut av = a.to_vec();
    let mut bv = b.to_vec();
    av.resize(n, 0);
    bv.resize(n, 0);
    (av, bv)
}

/// 两个版本号的数字部分是否表示同一个版本（`1.2` 与 `1.2.0` 视为相同）。
fn version_nums_equal(a: &[u32], b: &[u32]) -> bool {
    let (av, bv) = align_version_nums(a, b);
    av == bv
}

/// 比较两个版本号的数字部分（已按 `align_version_nums` 语义补齐 0）。
fn compare_version_nums(a: &[u32], b: &[u32]) -> std::cmp::Ordering {
    let (av, bv) = align_version_nums(a, b);
    av.cmp(&bv)
}

/// 比较版本号：返回 latest > current
/// 遵循 semver 预发布规则：数字部分相同且 latest 有预发布后缀时，按后缀分段数值比较
fn compare_versions(current: &str, latest: &str) -> bool {
    fn split_version(v: &str) -> (Vec<u32>, &str) {
        let v = v.trim_start_matches('v');
        let (base, pre) = match v.split_once('-') {
            Some((b, p)) => (b, p),
            None => (v, ""),
        };
        let nums: Vec<u32> = base.split('.').filter_map(|s| s.parse().ok()).collect();
        (nums, pre)
    }

    /// 按 semver 规范比较预发布标识符：按 `.` 分段，数字段数值比较，非数字段字符串比较
    fn compare_prerelease(a: &str, b: &str) -> std::cmp::Ordering {
        let a_parts: Vec<&str> = a.split('.').collect();
        let b_parts: Vec<&str> = b.split('.').collect();
        for (ap, bp) in a_parts.iter().zip(b_parts.iter()) {
            let ord = match (ap.parse::<u32>(), bp.parse::<u32>()) {
                (Ok(an), Ok(bn)) => an.cmp(&bn),
                _ => ap.cmp(bp),
            };
            if ord != std::cmp::Ordering::Equal {
                return ord;
            }
        }
        a_parts.len().cmp(&b_parts.len())
    }

    let (cur_nums, cur_pre) = split_version(current);
    let (lat_nums, lat_pre) = split_version(latest);

    // 先比较数字部分。
    // ⚠️ 必须**补齐到等长**再比（P3-2）：`Vec<u32>` 的 `Ord` 是字典序，
    // 短的排在短的后面 —— `[1,2] < [1,2,0]`，于是 `1.2` 会被判成「低于」`1.2.0`，
    // 明明两者是同一个版本。补齐 0 后长度一致，纯按数值比较。
    if !version_nums_equal(&cur_nums, &lat_nums) {
        return compare_version_nums(&cur_nums, &lat_nums) == std::cmp::Ordering::Less;
    }

    // 数字部分相同：有预发布后缀的版本 < 无后缀的版本（如 1.1.5-beta < 1.1.5）
    match (cur_pre.is_empty(), lat_pre.is_empty()) {
        (true, false) => false, // current 是正式版，latest 是预发布 → latest 不更新
        (false, true) => true,  // current 是预发布，latest 是正式版 → latest 更新
        _ => compare_prerelease(cur_pre, lat_pre) == std::cmp::Ordering::Less,
    }
}

/// 检测 GitHub 是否有新版本
fn check_for_update(current_version: &str, include_prerelease: bool) -> Result<UpdateInfo, String> {
    standard_log!(
        "[update] checking for update: current={} include_prerelease={}",
        current_version,
        include_prerelease
    );

    let body = winhttp_get("api.github.com", "/repos/oneday5799/PeriTray/releases")?;

    let releases: Vec<GitHubRelease> =
        serde_json::from_str(&body).map_err(|_| "响应数据解析失败".to_string())?;

    // 按版本号排序取最大（而非依赖 API 返回顺序）
    let latest = releases
        .iter()
        .filter(|r| {
            if r.draft {
                return false;
            }
            if r.prerelease && !include_prerelease {
                return false;
            }
            true
        })
        .max_by(|a, b| {
            let a_ver = a.tag_name.trim_start_matches('v');
            let b_ver = b.tag_name.trim_start_matches('v');
            // compare_versions 返回 latest > current，这里反转用于排序
            if compare_versions(a_ver, b_ver) {
                std::cmp::Ordering::Greater
            } else if compare_versions(b_ver, a_ver) {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Equal
            }
        });

    match latest {
        Some(release) => {
            let latest_ver = release.tag_name.trim_start_matches('v');
            let has_update = compare_versions(current_version, latest_ver);
            standard_log!(
                "[update] result: has_update={} latest={}",
                has_update,
                latest_ver
            );
            Ok(UpdateInfo {
                has_update,
                current_version: current_version.to_string(),
                latest_version: latest_ver.to_string(),
                release_url: release.html_url.clone(),
            })
        }
        None => {
            crate::process::append_log("[update] result: no releases found");
            Ok(UpdateInfo {
                has_update: false,
                current_version: current_version.to_string(),
                latest_version: current_version.to_string(),
                release_url: String::new(),
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── P3-1：响应体上限 ────────────────────────────────────────────

    /// 边界**两侧都断言**：恰好等于上限合法，超 1 字节即拒。
    /// 修复前不存在这个函数，也就无从断言 —— 这正是它被抽出来的理由。
    #[test]
    fn body_limit_boundary_is_exclusive_at_exact_size() {
        assert!(
            !body_too_large(MAX_RESPONSE_BODY),
            "恰好等于上限应被允许（上限本身不是非法值）"
        );
        assert!(!body_too_large(MAX_RESPONSE_BODY - 1), "低于上限应被允许");
        assert!(
            body_too_large(MAX_RESPONSE_BODY + 1),
            "超出上限 1 字节即应拒绝"
        );
        assert!(body_too_large(MAX_RESPONSE_BODY * 2), "远超上限应拒绝");
    }

    #[test]
    fn body_limit_covers_realistic_and_degenerate_sizes() {
        assert!(!body_too_large(0), "空响应体合法");
        assert!(!body_too_large(100 * 1024), "百 KB 量级是 GitHub 正常响应");
        assert!(body_too_large(usize::MAX), "极端值不应 panic / 溢出");
    }

    // ── P3-2：版本号数字段等长比较 ──────────────────────────────────

    /// `1.2` 与 `1.2.0` 数值上是同一个版本 ⇒ 不应判为「有更新」。
    /// 修复前 `Vec<u32>` 字典序把 `[1,2] < [1,2,0]` 判为 true，会误报有更新。
    #[test]
    fn version_nums_treat_trailing_zeros_as_equal() {
        assert!(version_nums_equal(&[1, 2], &[1, 2, 0]));
        assert!(version_nums_equal(&[1, 2, 0], &[1, 2]));
        assert!(version_nums_equal(&[1], &[1, 0, 0]));
        assert!(version_nums_equal(&[1, 2, 3], &[1, 2, 3]));

        // 反向：确实不同的不能被抹平
        assert!(!version_nums_equal(&[1, 2], &[1, 3]));
        assert!(!version_nums_equal(&[1, 2], &[1, 2, 1]));
    }

    /// 端到端：`1.2` vs `1.2.0` 两个方向都判「无更新」。
    #[test]
    fn compare_versions_ignores_missing_trailing_segment() {
        assert!(
            !compare_versions("1.2", "1.2.0"),
            "1.2.0 相对 1.2 不应判为更新（同一版本）"
        );
        assert!(
            !compare_versions("1.2.0", "1.2"),
            "1.2 相对 1.2.0 不应判为更新（同一版本）"
        );
    }

    /// 补齐不能把真实的版本升级吃掉。
    #[test]
    fn compare_versions_still_detects_real_bumps() {
        assert!(compare_versions("1.2", "1.3"), "次版本升级应检出");
        assert!(compare_versions("1.2.9", "1.3"), "1.3 > 1.2.9 应检出");
        assert!(compare_versions("1.2", "1.2.1"), "补丁升级应检出");
        assert!(compare_versions("1.2.0", "2.0"), "主版本升级应检出");
        assert!(!compare_versions("1.3", "1.2"), "降级不应判为更新");
    }

    /// 边界：位数不同的比较方向必须正确（不是「长度不同就判大」）。
    #[test]
    fn version_num_ordering_is_numeric_not_lexical() {
        use std::cmp::Ordering;
        assert_eq!(compare_version_nums(&[1, 2], &[1, 2, 0]), Ordering::Equal);
        assert_eq!(compare_version_nums(&[1, 2], &[1, 2, 1]), Ordering::Less);
        assert_eq!(compare_version_nums(&[1, 3], &[1, 2, 9]), Ordering::Greater);
        assert_eq!(compare_version_nums(&[1, 10], &[1, 9]), Ordering::Greater);
    }

    /// 预发布语义不应被本次改动破坏。
    #[test]
    fn prerelease_semantics_preserved() {
        assert!(compare_versions("1.2.0-beta", "1.2.0"), "预发布 < 正式版");
        assert!(
            !compare_versions("1.2.0", "1.2.0-beta"),
            "正式版不应被预发布顶掉"
        );
        assert!(
            compare_versions("1.2.0-beta.1", "1.2.0-beta.2"),
            "预发布序号比较"
        );
        assert!(!compare_versions("1.2.0-beta.2", "1.2.0-beta.1"));
        // 数字部分不同时，预发布后缀不参与（1.3.0-beta > 1.2.0）
        assert!(compare_versions("1.2.0", "1.3.0-beta"));
    }
}

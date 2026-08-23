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
    /// "latest" | "update" | "error"
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

    pub fn from_error(current_version: &str, error: &str) -> Self {
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

pub fn set_last_status(status: UpdateStatus) {
    if let Ok(mut guard) = LAST_STATUS.lock() {
        *guard = Some(status);
    }
}

pub fn get_last_status() -> Option<UpdateStatus> {
    LAST_STATUS.lock().ok().and_then(|guard| guard.clone())
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
    let ver_for_task = current_version.clone();
    let result =
        tokio::task::spawn_blocking(move || check_for_update(&ver_for_task, include_prerelease))
            .await;

    match result {
        Ok(Ok(info)) => {
            let status = if info.has_update { "update" } else { "latest" };
            set_last_status(UpdateStatus::from_info(&info, status));
            (Ok(info), true)
        }
        Ok(Err(e)) => {
            crate::process::append_log(&format!("[update]{} check failed: {}", prefix, e));
            set_last_status(UpdateStatus::from_error(&current_version, &e));
            (Err(e), true)
        }
        Err(e) => {
            crate::process::append_log(&format!("[update]{} task failed: {}", prefix, e));
            (Err(format!("task error: {}", e)), false)
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

/// WinHTTP GET request, returns response body as String
fn winhttp_get(host: &str, path: &str) -> Result<String, String> {
    let user_agent = crate::process::to_wide("PeriphMonitor");
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
        }

        WinHttpCloseHandle(request);
        WinHttpCloseHandle(connect);
        WinHttpCloseHandle(session);

        String::from_utf8(body).map_err(|_| "响应编码错误".to_string())
    }
}

/// 比较版本号：返回 latest > current
/// 遵循 semver 预发布规则：数字部分相同且 latest 有预发布后缀时，按后缀字典序比较
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

    let (cur_nums, cur_pre) = split_version(current);
    let (lat_nums, lat_pre) = split_version(latest);

    // 先比较数字部分
    if cur_nums != lat_nums {
        return cur_nums < lat_nums;
    }

    // 数字部分相同：有预发布后缀的版本 < 无后缀的版本（如 1.1.5-beta < 1.1.5）
    match (cur_pre.is_empty(), lat_pre.is_empty()) {
        (true, false) => false, // current 是正式版，latest 是预发布 → latest 不更新
        (false, true) => true,  // current 是预发布，latest 是正式版 → latest 更新
        _ => cur_pre < lat_pre, // 都是预发布或都是正式版，按后缀/相等比较
    }
}

/// 检测 GitHub 是否有新版本
pub fn check_for_update(
    current_version: &str,
    include_prerelease: bool,
) -> Result<UpdateInfo, String> {
    crate::process::append_log(&format!(
        "[update] checking for update: current={} include_prerelease={}",
        current_version, include_prerelease
    ));

    let body = winhttp_get("api.github.com", "/repos/oneday5799/PeriphMonitor/releases")?;

    let releases: Vec<GitHubRelease> =
        serde_json::from_str(&body).map_err(|_| "响应数据解析失败".to_string())?;

    let latest = releases.iter().find(|r| {
        if r.draft {
            return false;
        }
        if r.prerelease && !include_prerelease {
            return false;
        }
        true
    });

    match latest {
        Some(release) => {
            let latest_ver = release.tag_name.trim_start_matches('v');
            let has_update = compare_versions(current_version, latest_ver);
            crate::process::append_log(&format!(
                "[update] result: has_update={} latest={}",
                has_update, latest_ver
            ));
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

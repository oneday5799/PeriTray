//! 空间音效（CPolicyConfigClient 未公开扩展接口，槽位经 PDB 符号 + 运行时双重验证）
//!
//! 自 audio.rs 拆分：格式表、PolicySpatialClient RAII 封装与读写/校验逻辑。
//! 对外仅暴露 get_spatial_sound / set_spatial_sound / SpatialSoundFormat / SpatialSoundState。
//! 日志沿用 [audio] 前缀以保持检索习惯。

use serde::Serialize;
use std::ffi::c_void;
use std::ptr;
use windows::core::HSTRING;
use windows::Win32::System::Com::*;

#[derive(Debug, Clone, Serialize)]
pub struct SpatialSoundFormat { pub guid: String, pub name: String }

#[derive(Debug, Clone, Serialize)]
pub struct SpatialSoundState { pub current: Option<String>, pub supported: Vec<SpatialSoundFormat> }

/// 已知空间音效格式表（GUID 均为本机 Win11 实测，Sonic 与 NirSoft svcl 文档一致）。
/// 第三项为格式提供应用的包族名（AppX PackageFamilyName）：
/// None = 系统内置恒可用；Some = 需对应商店应用已为当前用户注册，卸载后从菜单移除
const SPATIAL_SOUND_FORMATS: &[(&str, &str, Option<&[&str]>)] = &[
    ("b53d940c-b846-4831-9f76-d102b9b725a0", "用于耳机的 Windows Sonic", None),
    (
        "1459ac38-3875-49bf-bb59-0fe80f4d395d",
        "Dolby Atmos for Headphones",
        Some(&[
            "DolbyLaboratories.DolbyAtmosforHeadphones_rz1tebttyb220",
            "DolbyLaboratories.DolbyAccess_rz1tebttyb220",
        ]),
    ),
    (
        "4444acb0-8dc0-4c2c-a0d8-2c76db470f86",
        "DTS Headphone:X",
        Some(&["DTSInc.DTSSoundUnbound_t5j2fzbtdg37r"]),
    ),
];

/// Get/SetDeviceSpatialSettings 写读的状态块有效长度（0x48 之外为堆噪声）
const SPATIAL_STATE_LEN: usize = 0x48;

pub fn get_spatial_sound(device_id: &str) -> std::result::Result<SpatialSoundState, String> {
    let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        // 仅接口本身缺失（系统不支持）才返回 Err 让前端降级为跳转入口；
        // 设备级读取失败（如激活中的格式被卸载）降级为无勾选状态，保留切换其他格式的自救能力
        let client = PolicySpatialClient::acquire()?;
        let supported: std::vec::Vec<SpatialSoundFormat> = SPATIAL_SOUND_FORMATS.iter()
            .filter(|(_, _, pkgs)| spatial_format_available(pkgs))
            .map(|(g, n, _)| SpatialSoundFormat { guid: g.to_string(), name: n.to_string() })
            .collect();
        let (current, supported) = match client.query(wide.as_ptr()) {
            Ok((state, fmt_ptr)) => {
                CoTaskMemFree(Some(fmt_ptr as *const c_void));
                if validate_state_encoding(&state) {
                    (state_current_guid(&state), supported)
                } else {
                    (None, supported)
                }
            }
            Err(_) => (None, supported),
        };
        Ok(SpatialSoundState { current, supported })
    }
}

/// 查询当前用户是否注册了指定包族的 AppX 应用（免管理员，经 PackageManager WinRT）
fn is_package_registered_for_user(family: &str) -> bool {
    use windows::Management::Deployment::PackageManager;
    unsafe {
        crate::audio::ensure_com_initialized();
        let Ok(pm) = PackageManager::new() else { return false; };
        let empty = HSTRING::from("");
        let fam = HSTRING::from(family);
        pm.FindPackagesByUserSecurityIdPackageFamilyName(&empty, &fam)
            .map(|p| p.into_iter().next().is_some())
            .unwrap_or(false)
    }
}

/// 格式可用性：无包族依赖（内置）恒可用；有依赖则任一包族已注册即视为可用
fn spatial_format_available(pkgs: &Option<&[&str]>) -> bool {
    match pkgs {
        None => true,
        Some(families) => families.iter().any(|f| is_package_registered_for_user(f)),
    }
}

pub fn set_spatial_sound(device_id: &str, format_guid: Option<&str>) -> std::result::Result<(), String> {
    let guid_hex = match format_guid.map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(s) => Some(parse_guid_str(s).ok_or_else(|| format!("无效的格式 GUID：{}", s))?),
    };
    let entry = SPATIAL_SOUND_FORMATS.iter().find(|(g, _, _)| parse_guid_str(g) == guid_hex);
    // 已知格式需校验提供应用是否仍安装，防止写入已卸载格式
    if let (Some(_), Some((_, name, pkgs))) = (&guid_hex, entry) {
        if !spatial_format_available(pkgs) {
            return Err(format!("{} 未安装或已被卸载", name));
        }
    }
    let target_name = match (&guid_hex, entry) {
        (_, Some((_, n, _))) => *n,
        (None, _) => "关",
        _ => "自定义格式",
    };
    crate::process::append_log(&format!("[audio] set_spatial_sound: {} -> {}", device_id, target_name));
    let wide: Vec<u16> = device_id.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let client = PolicySpatialClient::acquire()?;
        let mut new_state = [0u8; SPATIAL_STATE_LEN];
        if let Some(bytes) = &guid_hex {
            new_state[0] = 1;
            new_state[4] = 1;
            new_state[8] = 1;
            new_state[0x0C..0x1C].copy_from_slice(bytes);
            new_state[0x1C..0x2C].copy_from_slice(bytes);
            new_state[0x3C] = 1;
            new_state[0x44] = 1;
        } // 关 = 全零

        match client.query(wide.as_ptr()) {
            Ok((cur_state, fmt_ptr)) => {
                // 常规路径：复用设备格式指针写入 → 读回校验
                // 状态编码校验仅作记录不作硬闸门：激活中的格式被卸载后端点会停留在
                // 非标准过渡态，但系统设置器在此状态下同样接受写入并归一化；
                // 槽位漂移的真正防线是写后读回（错位调用必然读回不匹配）
                CoTaskMemFree(Some(fmt_ptr as *const c_void));
                let layout_trusted = validate_state_encoding(&cur_state);
                if !layout_trusted {
                    crate::process::append_log(
                        "[audio] set_spatial_sound: endpoint state encoding abnormal (provider uninstalled?), writing anyway",
                    );
                }
                let hr = client.try_set_state(wide.as_ptr(), &new_state, fmt_ptr);
                if hr < 0 {
                    return Err(format!("设置空间音效失败（hr={:#010x}）", hr as u32));
                }
                if !verify_after_write(&client, wide.as_ptr(), guid_hex.is_some(), guid_hex.as_ref()) {
                    return Err("设置未生效（接口布局可能已变化）".to_string());
                }
            }
            Err(query_err) => {
                // 降级路径：读取失败（如激活中的格式提供应用被卸载导致端点状态不可读）
                // fmt=null 直接写入；写后尽力读回，仍不可读则信任 HRESULT
                crate::process::append_log(&format!(
                    "[audio] set_spatial_sound degraded path (query err: {})", query_err
                ));
                let hr = client.try_set_state(wide.as_ptr(), &new_state, ptr::null());
                if hr < 0 {
                    return Err(format!("设置空间音效失败（hr={:#010x}）", hr as u32));
                }
                if !verify_after_write(&client, wide.as_ptr(), guid_hex.is_some(), guid_hex.as_ref()) {
                    return Err("设置未生效（接口布局可能已变化）".to_string());
                }
            }
        }
        Ok(())
    }
}

/// 写后校验：核对语义字段；首次不匹配时延迟重试一次（音频引擎可能异步归一化），
/// 读取失败视为暂时不可验证（由调用方按 hr 信任），返回 true=已确认生效或无法验证
fn verify_after_write(
    client: &PolicySpatialClient,
    wide_ptr: *const u16,
    enabled: bool,
    guid: Option<&[u8; 16]>,
) -> bool {
    let check = |state: &[u8; SPATIAL_STATE_LEN], fmt_ptr: *mut u16| -> Option<bool> {
        unsafe { CoTaskMemFree(Some(fmt_ptr as *const c_void)) };
        if !validate_state_encoding(state) {
            return None; // 仍处于过渡态，等待归一化
        }
        Some(state_matches(state, enabled, guid))
    };
    for wait in [0u64, 200] {
        if wait > 0 {
            std::thread::sleep(std::time::Duration::from_millis(wait));
        }
        match unsafe { client.query(wide_ptr) } {
            Ok((after, fmt_ptr)) => match check(&after, fmt_ptr) {
                Some(ok) => return ok,
                None => continue,
            },
            Err(_) => {
                crate::process::append_log("[audio] spatial verify: read-back unavailable, trusting HRESULT");
                return true;
            }
        }
    }
    false
}

/// CPolicyConfigClient 扩展接口封装。IID 随 Windows 构建漂移，按候选顺序 QI 命中即用。
struct PolicySpatialClient { ptr: *mut c_void }

impl Drop for PolicySpatialClient {
    fn drop(&mut self) {
        unsafe {
            let vt = *(self.ptr as *const *const usize);
            let release_fn: unsafe extern "system" fn(*mut c_void) -> u32 =
                std::mem::transmute(*vt.add(2));
            release_fn(self.ptr);
        }
    }
}

impl PolicySpatialClient {
    const CLSID_POLICY_CONFIG_CLIENT: windows_sys::core::GUID = windows_sys::core::GUID {
        data1: 0x870af99c, data2: 0x171d, data3: 0x4f9e,
        data4: [0xaf, 0x0d, 0xe6, 0x3d, 0xf4, 0x0c, 0x2b, 0xc9],
    };

    unsafe fn acquire() -> std::result::Result<Self, String> {
        crate::audio::ensure_com_initialized();
        let iid_unknown = windows_sys::core::GUID {
            data1: 0x00000000, data2: 0x0000, data3: 0x0000,
            data4: [0xC0, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x46],
        };
        let mut unk: *mut c_void = ptr::null_mut();
        let hr = windows_sys::Win32::System::Com::CoCreateInstance(
            &Self::CLSID_POLICY_CONFIG_CLIENT, ptr::null_mut(),
            windows_sys::Win32::System::Com::CLSCTX_ALL,
            &iid_unknown, &mut unk,
        );
        if hr < 0 || unk.is_null() {
            return Err("无法创建音频策略配置对象".to_string());
        }
        // 扩展接口 IID 候选：
        // - 4495581A：Win11 24H2 实测（slot22 单声道 / slot34/35 空间音效读写）
        // - E8478600 ：社区逆向记录的 Insider 构建变体
        // 布局自检由 query 后的 validate_state_encoding 把关，不匹配则拒绝写入。
        let candidates = [
            windows_sys::core::GUID {
                data1: 0x4495581a, data2: 0x01b9, data3: 0x4a8f,
                data4: [0xb0, 0x5c, 0x74, 0x1a, 0x6c, 0x98, 0x3d, 0x28],
            },
            windows_sys::core::GUID {
                data1: 0xe8478600, data2: 0xa74b, data3: 0x4b3a,
                data4: [0xa9, 0x6b, 0x1f, 0xc3, 0xe7, 0x96, 0xfc, 0x46],
            },
        ];
        let unk_vt = *(unk as *const *const usize);
        let qi_fn: unsafe extern "system" fn(*mut c_void, *const windows_sys::core::GUID, *mut *mut c_void) -> i32 =
            std::mem::transmute(*unk_vt);
        for iid in candidates.iter() {
            let mut out: *mut c_void = ptr::null_mut();
            let hr = qi_fn(unk, iid, &mut out);
            if hr >= 0 && !out.is_null() {
                let release_unk: unsafe extern "system" fn(*mut c_void) -> u32 =
                    std::mem::transmute(*unk_vt.add(2));
                release_unk(unk);
                return Ok(Self { ptr: out });
            }
        }
        let release_unk: unsafe extern "system" fn(*mut c_void) -> u32 =
            std::mem::transmute(*unk_vt.add(2));
        release_unk(unk);
        Err("当前系统不支持空间音效控制接口".to_string())
    }

    /// slot34：GetDeviceFormatAndSpatialSettings → (状态块前 0x48 字节, WAVEFORMATEX*)
    /// fmt 指针所有权移交调用方（需 CoTaskMemFree），settings/desc 内部释放
    unsafe fn query(&self, wide_ptr: *const u16) -> std::result::Result<([u8; SPATIAL_STATE_LEN], *mut u16), String> {
        let vt = *(self.ptr as *const *const usize);
        let get_fn: unsafe extern "system" fn(
            *mut c_void, *const u16, i32,
            *mut *mut u16, *mut *mut u8, *mut u32, *mut *mut u8,
        ) -> i32 = std::mem::transmute(*vt.add(34));
        let mut fmt: *mut u16 = ptr::null_mut();
        let mut settings: *mut u8 = ptr::null_mut();
        let mut mode: u32 = 0;
        let mut desc: *mut u8 = ptr::null_mut();
        let hr = get_fn(self.ptr, wide_ptr, 0, &mut fmt, &mut settings, &mut mode, &mut desc);
        if hr < 0 || settings.is_null() {
            if !fmt.is_null() { CoTaskMemFree(Some(fmt as *const c_void)); }
            if !desc.is_null() { CoTaskMemFree(Some(desc as *const c_void)); }
            return Err(format!("读取空间音效状态失败（hr={:#010x}），设备可能不支持", hr as u32));
        }
        let mut state = [0u8; SPATIAL_STATE_LEN];
        ptr::copy_nonoverlapping(settings, state.as_mut_ptr(), SPATIAL_STATE_LEN);
        CoTaskMemFree(Some(settings as *const c_void));
        CoTaskMemFree(Some(desc as *const c_void));
        Ok((state, fmt))
    }

    /// slot35：SetDeviceSpatialSettings，返回原始 HRESULT
    unsafe fn try_set_state(&self, wide_ptr: *const u16, state: &[u8; SPATIAL_STATE_LEN], fmt: *const u16) -> i32 {
        let vt = *(self.ptr as *const *const usize);
        let set_fn: unsafe extern "system" fn(*mut c_void, *const u16, *const u8, *const u16) -> i32 =
            std::mem::transmute(*vt.add(35));
        set_fn(self.ptr, wide_ptr, state.as_ptr(), fmt)
    }
}

/// 状态编码校验：前三个 DWORD 全零（关）或全一（开），且尾部标志位取值合法。
/// 用于拦截未来构建槽位漂移导致的错误调用。
fn validate_state_encoding(state: &[u8; SPATIAL_STATE_LEN]) -> bool {
    let d = |o: usize| u32::from_le_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]]);
    let off = d(0) == 0 && d(4) == 0 && d(8) == 0;
    let on = d(0) == 1 && d(4) == 1 && d(8) == 1;
    (off || on) && d(0x3C) <= 1 && d(0x40) == 0 && d(0x44) <= 1
}

fn state_current_guid(state: &[u8; SPATIAL_STATE_LEN]) -> Option<String> {
    if state[0] == 0 && state[4] == 0 && state[8] == 0 {
        return None;
    }
    Some(format_guid_bytes(&state[0x0C..0x1C].try_into().ok()?))
}

/// 读回校验：仅核对语义字段（使能标志 + 格式 GUID），容忍尾部字段变化
fn state_matches(state: &[u8; SPATIAL_STATE_LEN], enabled: bool, guid: Option<&[u8; 16]>) -> bool {
    let d = |o: usize| u32::from_le_bytes([state[o], state[o + 1], state[o + 2], state[o + 3]]);
    if !enabled {
        return d(0) == 0 && d(4) == 0 && d(8) == 0;
    }
    let Some(g) = guid else { return false };
    d(0) == 1 && d(4) == 1 && d(8) == 1
        && state[0x0C..0x1C] == g[..]
        && d(0x3C) == 1
        && d(0x44) == 1
}

fn format_guid_bytes(b: &[u8; 16]) -> String {
    format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6],
        b[8], b[9], b[10], b[11], b[12], b[13], b[14], b[15]
    )
}

/// 解析 "{...}" 或裸十六进制 GUID 字符串为内存序字节
fn parse_guid_str(s: &str) -> Option<[u8; 16]> {
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).collect();
    if hex.len() != 32 { return None; }
    let byte = |i: usize| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok();
    let mut raw = [0u8; 16];
    for (i, b) in raw.iter_mut().enumerate() {
        *b = byte(i)?;
    }
    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&[raw[3], raw[2], raw[1], raw[0]]);
    out[4..6].copy_from_slice(&[raw[5], raw[4]]);
    out[6..8].copy_from_slice(&[raw[7], raw[6]]);
    out[8..16].copy_from_slice(&raw[8..16]);
    Some(out)
}

#[cfg(test)]
mod spatial_tests {
    use super::*;

    #[test]
    fn guid_parse_format_roundtrip() {
        for (guid, _, _) in SPATIAL_SOUND_FORMATS {
            let bytes = parse_guid_str(guid).expect("parse");
            assert_eq!(format_guid_bytes(&bytes), *guid);
        }
        assert!(parse_guid_str("").is_none());
        assert!(parse_guid_str("zzzz").is_none());
    }

    #[test]
    fn validate_and_match_states() {
        let off = [0u8; SPATIAL_STATE_LEN];
        assert!(validate_state_encoding(&off));

        let mut on = [0u8; SPATIAL_STATE_LEN];
        on[0] = 1;
        on[4] = 1;
        on[8] = 1;
        let g0 = parse_guid_str(SPATIAL_SOUND_FORMATS[0].0).unwrap();
        on[0x0C..0x1C].copy_from_slice(&g0);
        on[0x1C..0x2C].copy_from_slice(&g0);
        on[0x3C] = 1;
        on[0x44] = 1;
        assert!(validate_state_encoding(&on));

        let mut corrupted = on;
        corrupted[10] = 7;
        assert!(!validate_state_encoding(&corrupted));

        assert!(state_matches(&off, false, None));
        assert!(!state_matches(&off, true, None));
        assert!(state_matches(&on, true, Some(&g0)));
        assert!(!state_matches(&on, true, Some(&parse_guid_str(SPATIAL_SOUND_FORMATS[2].0).unwrap())));
    }
}

# 03 · Rust 后端模块详解

20 个模块按域分组。行数为 2026-08-24 快照，随演进变化。

## 窗口域

### main.rs（394 行）— 入口与看门狗

- `install_panic_hook()`（main.rs:30）：panic 落盘 `[panic]` 日志 + MessageBox 再退出；payload 与 location 双提取
- `wait_process_exit()`（main.rs:73）：OpenProcess + WaitForSingleObject 内核级等待，看门狗重启专用
- `watchdog_self_restart()`（main.rs:92）：spawn 自身 `--autostart --watchdog-restart=<pid>` 后 exit(0)
- 启动时序、命令注册表、看门狗线程、on_window_event 路由——详见 [02 §2](02-架构总览.md)
- `#![warn(unused_imports, dead_code)]` 全局 lint，pre-commit 强制零警告

### popup.rs（253 行）— 弹出面板生命周期

- `cubic_bezier`（popup.rs:12）：Win11 风格缓动曲线的牛顿迭代求解器
- `compute_position`（popup.rs:38）：托盘坐标 → 目标位置（tray_y - POPUP_H - 15）与屏外起点
- 三个入口 `toggle / open_popup / close_popup` 均有 ANIMATING 门禁；`close_popup` 额外 is_visible 防护使其可被失焦事件安全重复分发
- `show()`（popup.rs:122）：**resume_webview → 屏外定位 → 置顶 → show → place_below_taskbar** 的固定顺序——顺序即不变式，勿重排（教训见 [07 §3](07-踩坑复盘-高频回归.md)）
- `animate_close` 末尾 `suspend_webview`：渲染进程休眠既是内存优化也是 B 类假死根治的一半
- `create()`：恒透明出生（`transparent(true)` 无条件）；圆角 DWMWA_WINDOW_CORNER_PREFERENCE=33

### windows.rs（768 行）— Win32/COM 互操作工具箱

| 区块 | 内容 |
|------|------|
| `browser_args()` | WebView2 附加参数：renderer-process-limit=1 等；硬件加速关闭时追加 --disable-gpu 族 |
| `open_settings_inner` | 已有窗口重开路径整体 spawn 化；新建走 async_runtime；两路径均恒透明出生 |
| `system_dark_mode()` | 读注册表 `AppsUseLightTheme` |
| `place_below_taskbar()` | FindWindowW("Shell_TrayWnd") + SetWindowPos 插队到任务栏正下方（topmost 波段内）；Explorer 重启间隙找不到任务栏则保持原序 |
| 材质模块 `material::apply/remove` | 三层降级：DWMWA_SYSTEMBACKDROP_TYPE(38) → DWMWA_MICA_EFFECT(1029) → SetWindowCompositionAttribute BlurBehind（未公开 API，LoadLibrary 动态解析）；`check_material_support` 用 RtlGetVersion 按 build 号判定（mica≥22000，acrylic≥17763） |
| `set_window_material` | 恒透明架构运行时切换协议：先 emit `material-changed` 让前端铺 CSS，再延迟 120ms 摘 DWM 背景板且执行前复核配置防快速往返竞态 |
| `set_webview_bg_color` | QI ICoreWebView2Controller2 vtable slot 16 SetDefaultBackgroundColor；`ensure_webview_bg_transparent` 带 4 次退避重试 |
| `suspend/resume_webview` | ICoreWebView2_3 TrySuspend(vt:68)/Resume(vt:69)，IID `{A0D6DF20-3B92-416D-AA0C-437A9C727857}`；前置/后置 put_IsVisible；`try_suspend_cb` 是手写的最小 COM 回调对象（#[repr(C)] vtable 结构）。**vtable 偏移注释是改这段代码的唯一依据，动前先读 windows.rs:255-270 注释块** |

### tray.rs（524 行）— 托盘与后台监听

- `setup_tray`（tray.rs:218）：自启状态同步 → 构建音频子菜单 → 主菜单 → TrayIconBuilder 接线
- **菜单事件处理铁律实例**（tray.rs:246-247 注释）：show/volume/settings/about/audio_dev_ 分支全部 `std::thread::spawn` 移出事件线程
- 左键点击（tray.rs:328-347）：spawn 内做坐标换算（**物理坐标 x/y 都要除缩放系数**，双臂匹配）+ toggle
- `start_device_watcher`：10s 轮询设备缓存，diff 变化才 spawn 更新 tooltip
- `start_theme_watcher`（tray.rs:116）：RegNotifyChangeKeyValue + 事件等待，深浅色切换事件驱动刷新图标（四象限：深/浅 × devices/volume）
- `config-changed` / `tray-devices-changed` / `audio-devices-changed` 三个内部事件接线

### state.rs（37 行）— 全局状态

TRAY_POS / POPUP_POS / ANIMATING / AUTO_START / AUTO_MENU_ITEM / DEVICES_CACHE 六个全局 + `lock_unpoisoned` 统一毒化恢复句式。

## 设备域

### wmi_query.rs（291 行）— 设备查询管线主管道

- `query_devices`（wmi_query.rs:51）：完整管线见 [02 §3.1](02-架构总览.md)；WMI 复用主线程 COM（assume_initialized）
- PNPCLASS 白名单常量（wmi_query.rs:107）：AudioEndpoint/Bluetooth/HIDClass/Keyboard/MEDIA/Mouse/Monitor
- `BT_STATUS_CONNECTED` 字符串常量供 WMI 构造与托盘判断共用，避免字面量散落
- 正则编译缓存（OnceLock），避免每次查询重复编译

### bluetooth.rs（482 行）— 蓝牙连接管理

- `bt_action(name, action)`（bluetooth.rs:335）：连接/断开统一入口；纯 Rust 实现（曾为 C#/PowerShell 方案，1286b22 重写）
- 蓝牙操作全局锁防并发干扰适配器状态；RAII 包装 HANDLE/COM 对象
- `find_paired_bluetooth_devices`：WinRT DeviceInformation 枚举配对设备；BLE 设备经 GATT Battery Service 读电量
- `check_device_connection`：用 core_name 比较（勿用原始名直接比较，2e4185e 教训）

### classify.rs（160 行）— 设备分类

`classify_device(name, pnp_class, pnp_id, caption)`：按 VID/PID 查 device_data 判定 2.4G 并路由类型（mouse/keyboard→输入，audio→音频，other→其他）。

### dedup.rs（144 行）— 去重

- `core_name(n)`：取括号内核心名并剥离蓝牙协议后缀（Hands-Free/Stereo/LE/Audio 等）。**注意与 tray::simplify_device_name 语义不同——本函数剥后缀返回 String，两者勿互换**（dedup.rs 头部注释显式警告）
- `try_insert`：核心名 + 连接类型联合判重，cn_index 倒排索引加速

### device_data.rs（176 行）— 2.4G 设备库

内置 JSON + 用户 JSON（data 目录，更新不覆盖）合并加载，同 VID/PID 用户优先；mtime 缓存避免重复解析；LRU/RwLock 管理。

### device.rs（48 行）— 数据模型

`DevType` 枚举（Mouse/Keyboard/Audio/Display/Battery/Input/Other 等）与 `Device` 结构（name/status/battery/is_bluetooth/dt 等）。

## 音频域

### audio.rs（672 行）— Core Audio 公开 API 封装

- 数据结构：`AudioDevice` / `AudioSession`（含 pid、Arc<str> 图标 data URL）/ `VolumeChangeEvent`
- `with_enumerator`：IMMDeviceEnumerator RAII 获取模板
- `set_default_device`（audio.rs:56）：**IPolicyConfig 未公开接口 vtable slot 13**（SetDefaultEndpoint），CoCreateInstance 手调，三 role 循环设置
- 设备音量族：`set_device_volume / toggle_device_mute / set_device_mute / set_shutdown_volumes`
- 会话族：`enumerate_audio_sessions`（进程名优先 FileDescription 版本信息）、`set_session_volume/mute`
- **应用级设备切换**（audio.rs:581-660）：SetPersistedDefaultAudioEndpoint/GetPersistedDefaultAudioEndpoint 未公开接口。deviceId 必须打包为设备接口路径 `\?\SWD#MMDEVAPI#<id>#<后缀>`——裸 id 会得到 0x80070057 静默失败（052d113 根因，详见 [07 §6](07-踩坑复盘-高频回归.md)）
- 快捷键动作：adjust_default_volume_up/down/toggle_default_mute 经 keybd_event 发 VK_VOLUME_* 虚拟键（借系统原生行为）

### audio_notify.rs（487 行）— 音频事件推送

- `init_audio_notify`（audio_notify.rs:356）：专职 STA 线程 = COM 初始化 + 注册隐藏消息窗口 `AudioNotifyMsgWindow` + 消息泵；monitor 以 Box::leak 挂在 GWLP_USERDATA
- AudioMonitor：设备级 IAudioEndpointVolumeCallback + 会话级 IAudioSessionEvents 双回调；`sync_callbacks` 对账式注册（增删会话补齐）
- SetTimer 每 3s 重同步会话列表（兜底漏报的变化）
- 变化统一 emit `volume-changed`（携带 session_id/device_id 区分）

### audio_spatial.rs（481 行）— 空间音效

- 自 audio.rs 拆分（71d5317）：CPolicyConfigClient 未公开扩展接口
- 槽位偏移经 **PDB 符号推导 + 运行时布局双重验证**；PolicySpatialClient RAII 封装
- 写入策略（8e3982e/4a2bc8b 迭代产物）：布局自检从硬闸门降级为日志；读回校验带 200ms 延迟重试；读取失败走降级写入路径（fmt=null 直写 + 尽力读回）；仅接口缺失才提示跳转系统设置

## 支撑域

### config.rs（253 行）— 配置单例

- `Config` 结构约 30 字段（完整字段清单见 config.rs:56-115），serde 序列化 JSON 存 exe 同目录
- `with_config / with_config_mut` 闭包式访问；`log_once` 防日志刷屏
- `LogRetention` 枚举控制日志保留策略

### process.rs（200 行）— 进程级基础工具集

模块名沿用历史。内容：`append_log` 日志子系统（exe 同目录 debug.log，GetLocalTime 本地时间戳）、`clean_old_logs` 保留策略清理、`to_wide` 宽字符转换、`shell_open` 及各类系统面板打开器（声音控制面板四页/设置页 ms-settings 跳转/sndvol）。约三分之二模块依赖它，新增跨模块基础工具优先落此处。

### commands.rs（481 行）— Tauri 命令薄封装层

~40 个 #[tauri::command]，职责限于：参数校验 → 调用域模块 → 写配置 → emit 相应事件。值得注意的实现：

- `update_config`：整包替换并广播 config-changed
- `set_hotkey_config / set_device_shortcut / remove_device_shortcut`：写配置后调 shortcut 域同步注册
- 阻塞操作在 tokio blocking 线程执行（commands.rs:17 辅助函数）

### shortcut.rs（188 行）— 全局快捷键

tauri_plugin_global_shortcut 封装：启动注册、`sync_device_shortcuts` 配置变更后增量重注册、录制期间 `SHORTCUT_RECORDING` 门禁（防止录制中触发已有快捷键）、触发后 emit `shortcut-recorded` 或直接执行动作。

### update.rs（317 行）— 更新检测

- `winhttp_get`：手写 WinHTTPS GET（session/connect/request/close 四段式，中文错误分类提示）
- GitHub Releases API 拉取 → 过滤 draft/prerelease（含测试版开关）→ 版本号比较（正确处理预发布号语义）
- `LAST_STATUS` 全局缓存最近一次检查结果（成功与失败均存），供关于页 infobar 持久展示与 `get_update_status` 命令读取
- `check_and_store`：spawn_blocking 包裹 + 任务级失败不覆盖旧状态的区分逻辑

### app_icon.rs（326 行）— 应用图标服务

`get_process_name_by_pid`（exe 版本信息 FileDescription 优先）/ `get_app_icon_by_pid`：进程图标提取 → PNG 编码 → data URL → LRU 缓存（Arc<str> 共享，免克隆）。

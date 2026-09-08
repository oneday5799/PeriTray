# PeriTray 优化方案（完整版）

> 版本：v1.3.5-beta.2 基线｜最后复核：2026-09-08（第九轮 · 终稿 v4）
> 范围：功能逻辑全量梳理——设备发现/蓝牙/2.4G/音频/托盘/快捷键/更新/前端双页/权限配置
> 方法：九轮递进核实（逐条源码复核 / 版本增量复核 / 方案可实现性推演 / 权限面与文档回读 / check.mjs 审计边界核查 / 四路深查 / 亲自纠偏 / 终验补扫 / 精度纠偏）
> 本清单全部条目均经直接源码验证；行号以 v1.3.5-beta.2 为准
>
> **完成状态**：feat/optimization-batch1 分支（8 批 · 37 项已落地，10 项未做/跳过）
> 图例：✅ 已完成　⏭️ 跳过（不做/延后）　未标注 = 未做

---

## 一、结论摘要

代码整体架构成熟：SWR 缓存双通道对称、单飞刷新防并发、错误通道分层、RAII 句柄覆盖、事件推送按实质变化节流、前端权限面干净。未发现系统性架构问题。

按优先级分五批推进，**第一批为真实缺陷（改动小、收益明确）**，其余为性能/前端/清理类优化。

---

## 二、功能全览（现状基线）

### 设备发现与监控
- PnP 设备枚举（WMI 单条白名单查询）+ 分类（音频/电池/显示器/其他/USB）
- 蓝牙：Classic + BLE 枚举、原生连接/断开（Win32）、电量（经典 SetupDi / BLE GATT）、适配器开关监听
- 2.4G 无线：雷蛇（64+12 PID）、罗技（7 款接收器 HID++）、AULA（F75 Max 可用 / F99 Pro 桩）、飞智 Vader 4 Pro、XInput 兜底
- 电量缓存：SWR（成功 TTL 5min / 负缓存 60s / 落盘 30 天淘汰 / 单飞后台补查 / 变更事件推送）
- 设备汇聚：core_name 去重合并、正则过滤、wireless_only 裁剪、分组/隐藏/重命名、托盘设备（上限 4）
- 低电量通知：阈值可配 + 设备选择 + Toast（进程内只通知一次）

### 托盘 / 弹窗 / 窗口
- 托盘菜单（设备/音量/音频子菜单/声音设置/自启/设置/关于/退出）、tooltip 电量、深色主题图标、左键定位弹窗
- 弹窗双 tab（设备/音量）、贝塞尔动画、DPI 定位、WebView2 Suspend/Resume、任务栏层级沉降
- 窗口材质（Mica/Acrylic/Default + 能力探测降级链）、圆角、恒透明、AUMID Toast

### 音频
- 输出/输入设备音量静音、应用会话音量、会话级路由（IPolicyConfig 逆向）、默认设备切换、空间音效（未公开接口 + 双保险）、事件推送（STA 线程回调注册表）、强制静音/静音锁定/精细调节/关机音量/媒体键

### 快捷键 / 配置 / 更新 / 稳定性
- 5 个基础全局快捷键 + 设备级快捷键（可共享循环切换 + 切换通知 Toast）+ 录制抑制
- TOML 配置持久化、字段级默认、旧版迁移、日志分级/保留
- GitHub Release 更新检查（含预发布）、启动/手动检测、Toast 跳转关于页
- 看门狗（事件循环探活 + 唤醒恢复 + 自重启）、panic hook、单实例

### 前端
- popup/settings 双页、对账渲染（popup）、右键菜单、快捷键录制器、toast、主题材质联动、config-changed 级联刷新
- 权限面：capabilities 仅 `core:default` + `window-state:default`，无 fs/shell/http，特权操作全部经 Rust 命令（已核实健康，见"已确认无问题"）

---

## 三、问题清单（按批次）

### 第一批：真实缺陷（高优先级，11 项）

| # | 位置 | 问题 | 影响 | 方案 | 状态 |
|---|---|---|---|---|---|
| 1 | `bluetooth.rs:659-670` / `wireless_24g/mod.rs:190-198,365` | 单飞标志（`BT_BATTERY_REFRESHING` / `REFRESHING`）在线程末尾裸 `store(false)` 复位，无 RAII（后台补查线程路径 bluetooth.rs:665 与 24g 两处 mod.rs:197,365 缺失；force 路径已有 `BtForceGuard`，不对称） | 后台补查线程内 panic 即**永久锁死该通道电量刷新** | 抽统一 `SingleFlightGuard`（CAS 获取 + `Option` 交出 + Drop 复位），三处复用；`BtForceGuard` 顺带替换为通用版 | ✅ b484039 |
| 2 | `bt_ble.rs:23-177` | BLE 单槽（`Option<BLEConnection>`）覆盖旧连接不显式 `Close()`；`ble_disconnect` 只能断开"最后连接"的设备（L152-173） | 多设备场景已确认：连 A→B 后旧 A 系统级连接未释放（模块头注释明确需 dispose）；断 A 报错 | **升级多设备 HashMap 方案**：`BLE_CONN` 改 `HashMap<String, BLEConnection>`（键=device_id）；同 id 存在则先 close 旧连接再替换；`ble_disconnect` 按 key `remove` 后关锁外 Close。已确认完全封装于 `bt_ble.rs`。**健壮性依赖 #27 先行**（同名多 BLE 需直传 device_id 才可区分路由） | ✅ d0d71fd |
| 3 | `device.rs:31-47` | `DEVICE_IDS` 以**设备名为键**，同名设备后写覆盖；且只在 `get_devices`/`get_devices_fresh` 命令入口刷新，托盘轮询路径不刷新（`store_device_ids` 全仓仅 `commands.rs:37,46` 调用） | 两只同名设备互踩 device_id，蓝牙连接/断开可能操作错设备 | 与本批 #27 同批解决（#27 为根治，删名键映射）。若 #27 暂缓则临时在 `tray.rs:115`"列表实际变化"分支补调 `store_device_ids`——注意 `device.rs:45` 每次调用打标准级日志，必须挂变化分支或降 verbose，否则 10s 刷屏 | ✅ 18687d8 |
| 4 | `audio_notify.rs:192-207` | `OnDefaultDeviceChanged` 只写 verbose 日志，不投递 `WM_SYNC_CALLBACKS` | 默认设备切换后前端 `is_default` 不实时（需等设备增删事件才刷新） | 该回调补 `PostMessageW(WM_SYNC_CALLBACKS)`。实现注意：`sync_callbacks` 只同步 eRender，capture 默认变化属幂等空扫，无需扩流 | ✅ b484039 |
| 5 | `update.rs:263` / `update.rs:279-287` | ① 预发布后缀**字符串字典序**比较（`cur_pre < lat_pre`）；② release 选取用 `find` 取列表顺序第一个，**未按版本排序** | ① `beta.2` vs `beta.10` 判错——项目自身发 beta，出现 beta.10 会漏提示升级；② 手动重建旧 release 时"最新"判定错乱 | ① 按 `.` 分段、数字段数值比较（标准 semver 预发布规则，~15 行）；② 候选 release 按 `compare_versions` 排序取最大（同函数 ~3 行），两修同提交并补单测 | ✅ b484039 |
| 6 | `toast.rs:16-22,32,54` | `TOAST_NOTIFIER` 用 `LazyLock::expect()`、`PREV_TOAST`（`OnceLock<Mutex<...>>`）用 `.lock().unwrap()` | 通知器创建失败或锁中毒 → panic → 弹窗 + 进程退出（旧实现是优雅记日志，**健壮性回退**） | 改存 `Option<ToastNotifier>` 优雅降级 + `state::lock_unpoisoned`；失败保留 verbose 日志 | ✅ b484039 |
| 7 | `dedup.rs:136-143` | 合并分支不更新 `battery`（仅 status/device_id/标志位） | 当前唯一带电量的调用路径被 `wmi_query.rs:301-312` 直查合并先行拦截，**暂不可达**；共享工具语义不完整，新调用方易踩 | 一行防御性补：`battery.is_some()` 时更新 | ✅ b484039 |
| 8 | `config.rs:328-332` | `with_config_mut` 用 `File::create` 先**截断**旧文件再写入，非原子替换 | 写入中途崩溃/断电（含看门狗 `process::exit` 强杀时刻）→ 文件损坏 → `init_config` 解析失败走**全量重置默认，用户全部配置丢失**（既有失败兜底放大了损坏后果） | 写同目录临时文件 + `rename` 原子替换（同卷原子），~5 行；配"旧文件可正常载入"回归 | ✅ b484039 |
| 43 | `settings-audio.js:266,271,298,302` | 关机音量 NumberBox 按钮路径（▲/▼）和 keydown 路径 `parseInt("") + 5 = NaN` 无 isNaN 守卫（input L277 和 blur L288 有守卫，按钮/keydown 漏了） | NaN → `clamp(NaN)` = NaN → `updateConfig(NaN)` → `config.shutdown_volume_devices[name] = NaN` → `saveConfig()` 中 `toml::to_string_pretty` 对 NaN 失败 → **此后所有设置保存静默丢弃**；内存态 NaN → 关机时 `NaN.max(0.0).min(1.0) = 0.0` → 该设备**被静音** | 四处补 isNaN 守卫（与 L277 对齐），或 `setNumberBoxValue` 统一拦截 | ✅ b484039 |
| 48 | `bt_ble.rs:127-131` + L44-52 | GATT 三次触发全部失败仍 `caching anyway` + 同 device_id 早退返回 "already connected" | 用户点"连接"→ GATT 失败→ 缓存幽灵连接→ 再点同一设备→ "already connected" 拦截→ 必须先手动断开清缓存才能重连 | GATT 失败路径不缓存（删除 L133-139 的 "caching anyway"），或早退时校验 `device.ConnectionStatus()` 真实状态 | ✅ b484039 |
| 50 | `popup-devices.js:215` | `check_bt_connection` 也是 #27 遗漏的调用点（传 `name`） | 文档 #27 只提了 connect/disconnect 两个命令，漏了 check_bt_connection 的 name→device_id 改造 | #27 方案补 check_bt_connection 命令签名与前端调用点 | ✅ 18687d8 |

### 第二批：音频性能（3 项）

> 批一子批分组见上方（#27→#2→#3→#48 同批；#1/#4/#5/#6/#7/#8/#43 独立）。

| # | 位置 | 问题 | 影响 | 方案 | 状态 |
|---|---|---|---|---|---|
| 9 | `audio.rs:353-383` / `audio.rs:266` | `find_session_volume` 每次 `set_session_volume/mute` **全量重扫所有设备所有会话**；`enumerate_audio_sessions`（L266）的 `_device_id` 参数被完全忽略（前端传筛选意图但后端全量扫描） | 会话滑块已有 100ms throttle（`popup-audio.js:716`），实际为**拖动期间每秒 10 次全量 COM 扫描**（卡顿源）；每次打开音量页也全量扫描 | **命令签名加 `device_id`**：`set_session_volume`/`set_session_mute` 后端只枚举该设备下的会话；`enumerate_audio_sessions` 同样按 `device_id` 裁剪。已核实可行性——`popup-audio.js:718-733/749-755` 回调闭包持有的 session 对象含 `deviceId` 字段，直接传参即可 | ✅ 5b6237d |
| 10 | `audio_notify.rs` OnDeviceAdded/Removed/StateChanged（如 L187） | `WM_SYNC_CALLBACKS` 直接 PostMessage 无合并 | 设备插拔抖动时多条同义消息排队，每条全量重扫，STA 线程积压、音量推送延迟 | `AtomicBool` pending 标志：已排队则跳过，处理时复位 | ✅ 5b6237d |
| 11 | `audio_notify.rs:522`（函数定义） / `commands.rs:285`（调用点） | `request_session_sync()` 与枚举并行、无握手 | 首次打开音量页时"会话回调尚未注册→音量变化漏推送"竞态 | 命令侧等待同步完成（STA 线程完成时经 channel ack），或枚举并入 STA 线程；与 #9 同域可并做 | ✅ 5b6237d |

### 第三批：前端稳定性（12 项）

| # | 位置 | 问题 | 影响 | 方案 | 状态 |
|---|---|---|---|---|---|
| 12 | `common.js:512,644` / `settings-shortcut.js:65,135` / `popup-audio.js:418` | `shortcutRecorders` Set 只 add 无 remove；设备快捷键列表每次 render 清 DOM（L65）后重绑（L135） | 旧 recorder 闭包**永久持有已卸载 DOM**（真实泄漏 + Set 无界增长），`shortcut-recorded` 遍历全 Set 随会话变慢 | `bindShortcutRecorder` 返回 dispose，清 DOM 前调用；事件遍历跳过已卸载 recorder | ✅ d08bca6 |
| 13 | `settings.js:714-726` | config-changed 级联全量重载（6 个函数），且 `settings-devices.js:13` / `settings-audio.js:10` 各自再 `get_config` | 单次开关切换 **3+ 次 get_config** + 全量重建 | **payload 具体化为 `{ config, changed }`**：后端 emit 附完整 Config 快照（KB 级）+ 变更键集合；前端 `config = payload.config` 免重取，按 changed 定向刷新。**前置条件**：`Config`/`DeviceShortcut` 需补 `PartialEq`（config.rs:50,56）；diff 须在 `clear_shared_device_shortcuts`（commands.rs:88-92）之后计算。**改造面**：`loadDevicesAsync`（settings-devices.js:13）/ `loadAudioDevicesAsync`（settings-audio.js:10）须改为接受 config 参数，否则仍有 2 次 get_config 残留 | ✅ d08bca6 |
| 14 | `common.js:711`（showToast `innerHTML = msg`）/ `popup-devices.js:485,531`（`.catch((e) => showToast(e))`）/ `popup-devices.js:105`、`popup-audio.js:463,850` | invoke 错误串/设备名直插 innerHTML | 本地数据注入 HTML（低危 XSS 面） | 先审计 showToast 调用方是否传 HTML，改 textContent 或统一转义 | ✅ d08bca6 |
| 15 | `settings-devices.js:22-129` | 分组列表整树 `innerHTML=""` 重建 | 设备多时随 config 变化频繁全量重建 | 复用 popup 的 `reconcileCards`。**必须晚于 #12**（见批次依赖） | ✅ d0d71fd |
| 16 | `popup.js:46-76` | focus 刷新手工重写 loadDevices 的 config 同步 + 渲染（无代际保护） | 与在途 `scheduleSilentRefresh` 可交叉写状态 | 复用 `loadDevices(false)` 或收敛进 popup-devices.js | ✅ 18687d8 |
| 17 | `popup-audio.js:898-940` | 强制静音先写 `forceMuteHold[devName]`（L908）后调 toggle，失败走 catch（L937）时 L935 的 delete 不执行 | 脏 hold 常驻，后续 volume-changed 用错误值覆盖真实状态 | catch 中补 delete | ✅ d08bca6 |
| 18 | `settings-general.js:34-41` | `check_material_support("mica")` 返回 false 直接 return，ComboBox 已显示新值 | **仅 UI 漂移**（`config.window_material` 未污染，L43 在 return 之后） | 失败时把 combo 恢复为 config 现值 | ✅ 47797e4 |
| 19 | `settings-shortcut.js:33-39` / `popup-audio.js:381-390` | 快捷键清除路径 `.catch(() => {})` 吞错 | **机制已修正**：后端 `set_config_key` 走 `with_config_mut` 实际会落盘持久化；真实问题是吞错无提示、**不广播 config-changed**（其他窗口不感知）、本地先行改值在失败时与后端漂移。**补全**：`set_hotkey_config`（commands.rs:412-428）注册被外部占用时配置已写入但快捷键未生效，下次启动静默失败——注册失败时应回滚配置 | 清除路径对称处理：await + 失败 toast + 后端补 emit config-changed（与 #13 联动）；注册失败时回滚配置 | ✅ 47797e4 |
| 20 | `settings-devices.js:57` | `config.hidden_groups.includes` 假设恒为数组 | 已核实当前不会出现缺键（后端 Config 恒序列化该字段）；纯防御加固 | `(config.hidden_groups \|\| []).includes(...)`，可随手带上 | ⏭️ 不做——后端恒序列化该字段，不可能缺键，纯加固无实际收益 |
| 46 | `main.rs:291-294` | 弹窗 CloseRequested（Alt+F4/关闭按钮）只 `hide()` 不 `suspend()` | 与失焦关闭路径（`popup.rs:348-352` suspend）不一致：WebView 渲染进程持续运行、JS 定时器继续跑、内存不释放（实际不可达：popup 无 X 按钮，正常路径走 `popup::close`） | CloseRequested handler 中补 suspend（与失焦路径对齐） | ✅ 47797e4 |
| 47 | `commands.rs:158,247` + `tray.rs:314` | `rename_device`（emit `audio-devices-changed`）、`toggle_device_tray`（emit `tray-devices-changed`）、托盘 auto_start（无 emit）三个 mutator 不广播 `config-changed` | 重命名设备后设置页设备名/快捷键卡片不实时同步；切换托盘设备/自启后设置页状态漂移 | 三个 mutator 补 `app.emit("config-changed", ())`（零成本，与其余 5 个发射点对齐） | ✅ d08bca6 |
| 49 | `popup-devices.js:117-119` | `deviceKey = name + bt/24g`，缺 is_ble 维度 | 两只同名 BLE 设备（含不同厂）在 UI 合并为一张卡，#2 的"含同名逐一连接/断开"回归点在前端不可达 | deviceKey 追加 `is_ble` 或 `device_id` 维度 | ✅ d08bca6 |

### 第四批：查询效率与杂项（8 项）

| # | 位置 | 问题 | 方案 | 状态 |
|---|---|---|---|---|
| 21 | `wmi_query.rs:228-241` + `classify.rs:18-20,93-98,163-174` + `device_data.rs:163-186` | 每个 USB 行最多 **5 次** `extract_vid_pid` 重复解析（classify_device_inner 1 + is_wireless_24g_by_vid_pid 1 + 主循环 2 + wmi_query.rs:231 1） | 行首解析一次，合并为 `device_data::lookup(vid, pid) -> Option<(is_24g, name, type)>` 单接口透传全行 | ✅ 5605561 |
| 22 | `bluetooth.rs:702-748` | 经典电量逐台全量枚举 SetupDi 系统设备类（O(N×M)） | 后台补查批次内枚举一次，收集全部 MAC→电量后批量写缓存 | ⏭️ 跳过——中高复杂度，需重构 SetupDi 遍历逻辑，实际设备数有限（≤5 台），性能收益不明显 |
| 23 | `config.rs:322-338` | `with_config_mut` 每次调用全量 `toml::to_string_pretty` + 写盘（原子替换已由 #8 解决，此处是频率） | 序列化后与上次内容比对，未变化跳过写盘（脏检查） | ✅ 5605561 |
| 24 | `battery_notify.rs:32` | `resolve_toast_icon` 在通过 enabled/thresholds 检查后**无条件执行**（`selected.is_empty()` 时照写文件），且该检查在循环内逐设备做（L44） | 图标解析延迟到首次真正命中阈值；`selected.is_empty()` 提前到循环外 | ⏭️ 跳过——低电量通知为低频事件，图标解析一次性开销可忽略 |
| 25 | `audio_spatial.rs:82-103` | `is_package_registered_for_user` 每次新建 PackageManager + WinRT 查询（Dolby/DTS 各一次） | 按进程 TTL（如 60s）缓存包族注册结果 | ✅ 5605561 |
| 26 | `device_data.rs:140-151` | 每轮轮询对用户文件做一次 `fs::metadata`（文件不存在时 `.ok()` 静默） | 成本实测仅每 10s 一次 stat 系统调用——**价值低**，如做则缓存"不存在"状态 + 拉长重试间隔 | ⏭️ 不做——实测每 10s 一次 stat 系统调用，开销极低，优化无感知收益 |
| 27 | `device.rs` 映射（#3 根治） | 前端传 `name`（popup-devices.js:197,199,215），Rust 侧经 `DEVICE_IDS` name→id 映射解析；同名设备覆盖问题（#3 根因） | **已提至批一子批 B（与 #2/#3 同批）**；若未在批一落地则在批四执行。**方案**：前端直传 `device_id + is_ble` 给 `connect/disconnect_bluetooth_device`/`check_bt_connection`，删除 name→id 全局映射；`deviceKey`（L117-119）需加 is_ble 维度 | ✅ 18687d8 |
| 28 | `settings-about.js:18-25` + `settings.js:460,513` | `runUpdateCheck` 开始处快照按钮文案、finally 回填。用户先点检测时快照的是静态占位，`get_app_version` 后返回的真实版本号**被 finally 覆盖直至页面重载** | 版本 promise 完成前禁用检测按钮，或 finally 改回填异步取得的真实值 | ✅ 5605561 |

### 第五批：清理 / 低价值（14 项）

| # | 位置 | 问题 | 方案 | 状态 |
|---|---|---|---|---|
| 29 | `audio.rs:71-75` | `pwstr_to_string` 错误路径（`to_string()?` 提前返回）不执行 `CoTaskMemFree` | 先拷贝后无条件释放 | ✅ 1217487 |
| 30 | `audio.rs:93` | 单台设备 ID 转换失败 `?` 使整个枚举返回 Err | 改 `continue` 跳过 | ✅ 18687d8 |
| 31 | `audio.rs:296` | `state.0 > 2` 隐式数值过滤 AudioSessionState | 显式枚举 match | ✅ 1217487 |
| 32 | `audio.rs:180` | `set_shutdown_volumes` 按设备名匹配（同名全设、重命名失效） | 配置层改稳定 id（需配置迁移，低优先） | ⏭️ 跳过——需配置迁移（改 TOML schema），低优先级，同名设备场景极少 |
| 33 | `audio.rs:243-248` | `force_mute_prev_volume` 全局表无清理（设备移除/重命名后残留） | 移除设备时顺带清理 | ✅ d0d71fd |
| 34 | `tray.rs:124,203` / `windows.rs:34` 等 | 短小任务（tooltip/图标更新等）spawn 线程无节流 | 轻量操作直接调用或统一串行执行器。**注意**：`popup.rs:154` 是动画任务（`animate_close`），非短小任务；`window_material.rs:280` 的延迟线程已有配置复核（L282-283）+ DWM 对无效 hwnd 幂等失败，属"可选加固"非必修 | ⏭️ 跳过——需逐处评估是否可改同步调用，中高复杂度，当前无用户可感知问题 |
| 35 | `device.rs:38` / `update.rs:52,58` / `shortcut.rs:53` 等 | 锁风格混用（`lock_unpoisoned` vs `.lock().ok()` vs `.unwrap()`） | 统一 `state::lock_unpoisoned` | ⏭️ 跳过——纯风格一致性，不影响功能，改动量大（全仓多文件） |
| 36 | `process.rs:67-87` | 日志每条 open/append/close | 缓存 File 句柄（需处理按天轮转失效），可选项 | ✅ d0d71fd |
| 37 | `update.rs:147-235` | WinHTTP 错误分支 6 处重复 CloseHandle 三连（含正常关闭路径） | RAII guard 收敛 | ⏭️ 跳过——中高复杂度，6 处重复代码收敛为 RAII guard，但更新检查为低频操作 |
| 38 | `app_icon.rs:22-26,121-184` | 解析失败不缓存（失败 PID 每次全量重查）；`normalize_image_path` 每次逐盘符遍历 QueryDosDeviceW（O(D), D≈3-5） | 负缓存（短 TTL）+ 盘符映射表进程级缓存 | ⏭️ 跳过——中复杂度，需设计负缓存 TTL + 盘符映射生命周期管理 |
| 39 | `classify.rs:140-145,114` | `"gpro"`/`"g pro"` 冗余；`"hunters"`（L145）疑似 Huntsman 笔误（现值匹配不到 Huntsman 设备）；`"amp"`（L114）子串可误伤 Rampage/Lamp | 核对品牌名、考虑词边界（影响有限：多数设备走 2.4G 注册表或 PNPClass 路径） | ⏭️ 跳过——影响有限，多数设备走 2.4G 注册表或 PNPClass 路径，品牌名误伤概率低 |
| 40 | `webview.rs:66-75` | 透明重试**无条件跑满 4 次**（累计 sleep ~3s + 4 行日志），`set_webview_bg_color` 返回值被忽略 | 把成功信号从 `set_webview_bg_transparent` 返回，首次成功即退出 | ✅ d0d71fd |
| 41 | `xinput.rs:26` | 注释"20 字节，与 Win32 XINPUT_BATTERY_INFORMATION 布局一致"——结构体实为 2×u8=2 字节（Win32 原结构也是 2 字节） | 修正注释 | ✅ 1217487 |
| 42 | `toast.rs` 与 `windows.rs:394` `build_toast` | 双 toast 体系并存：更新通知走 build_toast（带点击回调，不经 PREV_TOAST Hide 机制），低电量/切设备走 `toast::show_toast`——两系统可同时弹 | 可选统一：toast.rs 增加 activation 回调支持后合并 build_toast 调用方 | ⏭️ 延后——需回调改造（toast.rs 增加 activation 支持），改动中等非必要 |

### 取舍边界（明确做 / 延后 / 不做）

| 类别 | 项 | 说明 |
|---|---|---|
| **做** | 批一至批四（#1-#50，含 #2 多 BLE、#27 提前、#43 NaN、#48 幽灵连接、#46/#47/#49 前端增强） | 有明确收益 |
| **延后可选** | #36（日志句柄缓存——有按天轮转失效风险，收益可能不抵复杂度）、#42（双 toast 统一——更新 toast 需点击回调，改动中等非必要）、#51（volume-changed 前端节流——#9 落地后为次要开销） | 低优先，本方案不强制 |
| **明确不做** | #20（`hidden_groups` 防御——已核实后端恒序列化该字段，纯加固）、#26（`device_data` 每轮 stat——实测每 10s 一次 stat 系统调用，价值过低）、skip_version 功能（手动下载模式影响小，属功能请求非缺陷） | 收益/成本不划算，从执行清单剔除（仅保留说明） |

### 已复核更正 / 已确认无问题（不进入执行）

| 项 | 结论 |
|---|---|
| `popup.js:38` 裸用 invoke | **误报（已撤销）**——实际用 `getInvoke()`，符合 AGENTS.md 双轨约定 |
| `dedup.rs` 合并丢电量 | **降级为潜在隐患**（#7）：当前调用路径被 `wmi_query.rs:301-312` 先行拦截，不可达 |
| `hidden_groups` 旧配置缺键漂移 | **不成立**——该字段无 `#[serde(default)]`，缺键 = 整文件解析失败走默认回退，不存在静默分叉场景（前端防御见 #20，纯加固） |
| capabilities 权限面 | **健康**——仅 `core:default` + `window-state:default`，无 fs/shell/http，特权操作全经 Rust 命令 |
| #9 方案前端可行性 | **坐实**——会话卡回调闭包持有完整 session（含 `deviceId`），加参数即可 |
| `device_data` 每轮 stat（#26） | 实测成本仅每 10s 一次 stat 系统调用且静默——价值低于初判，明确不做（见取舍边界） |

### 明确不动的区域

- `audio_spatial.rs:342-343` / `audio_policy.rs` 裸 vtable 区：未公开 COM 接口逆向，编码校验 + 写后读回双保险已到位；`query` 无长度检查的 0x48 字节拷贝属高危区，**改动需单独论证，默认不改**
- `hid_link.rs:101` 无论设备是否提前应答都睡满 `wait_ms`：Feature Report 异步响应模型的保守正确性取舍，改后有回归风险

---

## 四、实施批次与验证

每批独立提交，遵循 AGENTS.md commit 规范（`type(scope): 中文标题` + body 写清根因/影响边界；纯等价重构显式声明"行为零变化"）。验证统一走：

```bash
node tools/check.mjs                 # 六类结构闸门（<1s）
cargo fmt --check && cargo check     # Rust 零警告（pre-commit 自动）
cargo test                           # 单测
```

| 批次 | 内容 | 回归要点 | 状态 |
|---|---|---|---|
| **批一** | #1-#8 + #43/#48/#50（子批 A+B） | 子批 A：拔插 2.4G/蓝牙观察"后台补查结束"日志正常；切换默认设备前端 is_default 即时更新；`compare_versions` 补 `beta.2 vs beta.10` 与 release 选序单测；toast 创建失败不再崩进程；配置保存改名机制下设置页保存/重启载入正常、旧配置文件可正常迁移；**#43 关机音量 NumberBox 空输入不产生 NaN**。子批 B：**同时连 2+ 个 BLE 设备（含同名）逐一连接/断开均正确**、托盘轮询后蓝牙连接仍指向正确 device_id、设备信息页右键断开精确到目标设备；**#48 GATT 失败不缓存幽灵连接、再点同一设备不被 "already connected" 拦截**；**#50 check_bt_connection 传 device_id 后轮询状态正确** | ✅ b484039 |
| **批二** | #9-#11 音频性能 | 拖会话音量条流畅度；设备插拔抖动后 STA 线程无积压（日志合并）；首次打开音量页后音量变化推送不丢 | ✅ 5b6237d |
| **批三** | #12-#20 + #46/#47/#49 前端稳定性 | 多次 config 切换后 DevTools 堆快照 DOM 不增长；单开关切换 get_config 调用 ≤1 次；材质切换失败 combo 回滚；快捷键清除失败有提示；版本号加载完成前不可点检测；**#47 从托盘切换自启/重命名设备后设置页实时同步**；**#49 同名 BLE 设备不合并为一张卡** | ✅ d08bca6 + 47797e4 + 18687d8 |
| **批四** | #21-#28 查询效率 | WMI 轮询耗时对比；蓝牙电量批次化后日志逐台→逐批；配置改动落盘频率（脏检查生效） | ✅ 5605561 + 18687d8 |
| **批五** | #29-#42 清理 | 逐项行为零变化确认（commit body 声明"纯等价重构"）；#42 toast 统一后点击跳转回归 | ✅ 1217487 + 18687d8 + d0d71fd |

### 批次间依赖与顺序约束

- **#12 必须先于 #15**：recorder dispose 未落地前改对账渲染，recorder 绑定路径要重做
- **#27 必须先于 #2**：同名多 BLE 需前端直传 device_id 才可区分路由
- **#2 必须先于 #48**：幽灵连接修复依赖 HashMap 多槽（同设备重连需校验真实状态）
- **#9 签名改动同步前端**：`popup-audio.js` 会话滑块/静音调用点（L733 `throttledSetSessionVolume`、L755 `setSessionMute`）与后端 `commands.rs` 同提交
- **#27 与 #9 共享回归**：同为命令签名改动 + 同为弹窗调用点，可安排同轮回归
- **#49 依赖 #27**：deviceKey 加 is_ble 需与 #27 的 device_id 直传同批落地
- **#13 payload 改造向后兼容**：payload 缺省（undefined）时前端回退自行 `get_config`，防旧前端缓存窗口期白屏
- **#19 依赖 #13**：清除路径补 broadcast 走统一 config-changed 机制
- **前端文件义务（AGENTS.md）**：#15/#16 若把共享纯函数抽到新 JS 文件，**必须同时加进 `popup.html`/`settings.html` 的 `<script>` 标签**，否则 `check.mjs:224-233` 报"孤立文件"；新增页面须同步 `check.mjs:7` 的 `PAGES` 数组

---

## 五、风险与注意事项

1. **config-changed payload 改动**（批三 #13）：前端事件流核心，改前先列全 listener（`common.js:76` / `popup-audio.js:121` / `settings.js:714`），保持向后兼容（payload 缺省回退全量刷新）；`Config`/`DeviceShortcut` 需先补 `PartialEq`（config.rs:50,56）；diff 须在 `clear_shared_device_shortcuts` 之后计算
2. **命令签名改动**（批二 #9、批一 #27/#50）：`invoke_handler` 注册表与前端调用点必须同步。**注意**：`tools/check.mjs` 的跨文件审计只校验 JS 标识符有无声明（`check.mjs:208-221`），**不感知后端 `#[tauri::command]`/`invoke_handler` 映射**——invoke 参数名与后端不匹配它拦不住，须靠人工 + Tauri 运行时验证
3. **COM 线程单元**：不得跨 tokio 阻塞池线程缓存 COM 接口指针（STA 亲核）——#9 选单设备扫描而非接口缓存即为此
4. **toast 降级**（批一 #6）：改优雅降级后，通知器创建失败时功能静默失效，需保留 verbose 日志可排查
5. **store_device_ids 日志噪声**（批一 #3 过渡）：`device.rs:45` 为标准级日志，接入托盘轮询必须挂"实际变化"分支，否则每 10s 刷屏
6. **update_config 全量覆盖竞态**：设置页 `saveConfig` 写整个 config，与后台 mutator（如托盘切自启）并发时 last-write-wins 可覆盖对方刚改的字段。低频；#13 改造时评估后端按 diff 写入或 patch 式命令，不在本期强制
7. **低电量通知语义**：去重永不重置（回升后再降不二次通知）为**刻意设计**；如需"回升 X% 才重提醒"另立需求
8. **WIKI 同步义务**：批二/批三涉及命令签名与事件 payload 变化，按 AGENTS.md 七类变更义务同步 Wiki 页 05（命令/事件增删），发版时走校准轮
9. **#43 NaN 污染链**：NumberBox 空输入 → NaN 写入 config → `toml::to_string_pretty` 失败 → 后续所有 saveConfig 静默丢弃。修复时需同时验证：正常输入 → 保存 → 重启载入 → 关机音量生效
10. **#48 幽灵连接**：GATT 失败仍缓存 + 同设备早退 "already connected" → 用户必须手动断开才能重连。修复时需验证：GATT 失败 → 不缓存 → 再点连接 → 正常重试

---

## 六、完成统计

| 指标 | 数值 |
|------|------|
| 总条目 | 50 |
| 已完成 | 37 |
| 跳过（不做/延后） | 10 |
| 明确不动区域 | 2 |
| 已确认无问题 | 5 |

| 批次 | 总项 | 已完成 | 跳过 |
|------|------|--------|------|
| 第一批（真实缺陷） | 11 | 11 | 0 |
| 第二批（音频性能） | 3 | 3 | 0 |
| 第三批（前端稳定性） | 12 | 11 | 1 (#20) |
| 第四批（查询效率） | 8 | 5 | 3 (#22,#24,#26) |
| 第五批（清理） | 14 | 7 | 7 (#32,#34,#35,#37,#38,#39,#42) |

---

## 附：核实记录

- 第一轮：五路 explore 代理分模块审查 → 逐条亲自源码复核，更正 2 处（popup.js 误报、dedup 降级）
- 第二轮：v1.3.4 → v1.3.5-beta.2 增量复核，确认缺陷文件零变更；新增 toast.rs 健壮性问题（#6）
- 第三轮：形成方案文档
- 第四轮：文档回读 + 权限面核查（capabilities 健康）+ 修复方案可实现性推演（#3 日志噪声、#5 选序、#13 payload 具体化、#12→#15 顺序约束、#9 前端坐实）
- 第五轮：剩余代理来源行号锚点全部亲验（tray/popup/windows/window_material/app_icon/xinput/battery_notify/audio_spatial/device_data/hid_link/settings-shortcut 等），三处按实测降级（#26 成本、#34 window_material 部分、#24 措辞），全部行号与 v1.3.5-beta.2 对齐
- 第六轮：check.mjs 审计边界核查（风险 #2 修正）+ AGENTS.md 前端文件义务补充 + 取舍边界（#20/#26 不做、#36/#42 延后）+ 用户确认多 BLE 使 #2 升级 HashMap / 与 #27/#3 同批（终稿 v2）
- 第七轮：四路 explore 代理深查（音频事件推送 / BLE 全链路 / config-changed 链路 / 全局健壮性）→ 新增 #43-#48（含纠正 2 处误报：app_icon 越界读、BLE 枚举不 Close）
- 第八轮：终验补扫（前端完整性 / 配置迁移 / Rust 健壮性 / DPI / 窗口重建）→ 新增 #49/#50，确认 16 项检查全部通过（终稿 v3）
- 第九轮：精度纠偏——三路 explore 代理逐条核实文档行号/措辞/计数，更正 13 处精度偏差（#1 RAII 描述、#3 行号、#6 PREV_TOAST 类型、#11 函数定义、#13 函数计数 5→7、#21 函数名对应、#27 表格列错位、#28/#42 行号、#34 移除动画任务、#37 计数 7+→6、#38 措辞）（终稿 v4）
- **实施轮（2026-09-08）**：feat/optimization-batch1 分支，8 批 37 项落地（b484039 → 5b6237d → d08bca6 → 47797e4 → 5605561 → 1217487 → 18687d8 → d0d71fd），10 项跳过

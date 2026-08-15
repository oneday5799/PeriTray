<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" alt="PeriphMonitor Logo">
</p>

<h1 align="center">PeriphMonitor</h1>

<p align="center">
  一款轻量级的 Windows 系统托盘外设监控工具，实时显示所有连接设备的状态信息。
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.80+-black?style=flat-square&logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/Tauri-2.x-blue?style=flat-square&logo=tauri" alt="Tauri">
  <img src="https://img.shields.io/badge/Platform-Windows%2010%2F11%20(x64%2FARM64)-0078d4?style=flat-square&logo=windows" alt="Platform">
  <img src="https://img.shields.io/badge/License-MIT-green?style=flat-square" alt="License">
</p>

---

## 简介

PeriphMonitor 是一款运行在 Windows 系统托盘中的轻量级外设监控工具。通过 WMI 查询、WinRT 蓝牙 API 和 windows_pnp 库实时检测所有连接的外设设备，以分类列表展示设备状态，并提供音量控制、蓝牙连接管理等功能。

## 功能

### 设备检测与管理

- 实时检测音频、USB、蓝牙、电池、显示器等设备
- 设备卡片显示连接类型标签（蓝牙/2.4G）、连接状态、电量百分比
- 设备按连接状态排序：已连接 > 已配对 > 未连接
- 设备重命名与隐藏（设备信息页和音量控制页独立操作）
- 设备分组管理，支持自定义分组和分组可见性控制
- 2.4G 无线设备自动识别，按 VID/PID 匹配并按设备类型归入对应分组
- 设备去重：同名设备保留有连接类型的版本（蓝牙/2.4G 优先于普通 USB）
- 正则表达式过滤，可编辑过滤规则

### 蓝牙

- 蓝牙设备显示连接/配对状态和电量百分比
  - BLE 设备：通过 GATT Battery Service (0x180F/0x2A19) 读取
  - BTC 设备：通过 windows_pnp 库读取 DEVPKEY_BLUETOOTH_BATTERY
- 蓝牙连接/断开操作（全局锁防止并发干扰适配器状态）
- 连接失败时 toast 提示，点击可跳转系统蓝牙设置页
- 使用系统蓝牙连接开关，支持跳转到 Windows 蓝牙设置页管理连接
- 显示无名称蓝牙设备开关（默认关闭）

### 音量控制

- 列出所有音频输出设备，支持切换默认设备、调节音量、静音
- 按应用查看/调节音量会话，音量滑块实时同步系统变化
- 音频设备音量滑块支持鼠标滚轮 1% 精准调节
- 音频事件完全事件驱动：IAudioEndpointVolumeCallback 实时回调音量变化，IMMNotificationClient 实时检测设备插拔
- 应用图标显示（从进程可执行文件提取，base64 PNG，按 PID 缓存）
- 音频设备右键菜单支持重命名、隐藏和设置快捷键（可为设备录制全局快捷键，一键快速切换默认输出设备）

### 系统托盘

- 左键弹出设备信息/音量控制页面（默认打开页面可在设置中切换）
- 托盘图标随默认打开页面切换：默认「音量控制」时显示音量图标
- 右键原生菜单：设备信息、音量控制、音频设备切换、声音设置、开机自启、设置、关于
- 音频设备切换子菜单：显示重命名后的名称，自动过滤已隐藏设备
- 图标悬停显示设备信息（状态、电量），最多支持 4 个设备，状态变化时自动更新
- Windows 声音设置快捷入口（音量合成器、播放设备、录制设备、声音、声音设置）

### 设置页

- **通用设置**：开机自启动、默认打开页面（托盘左键点击打开的页面）、音量控制快捷键（提高/降低/静音，支持全局快捷键录制）、硬件加速开关（默认关闭，关闭后 WebView2 减少 20-40MB GPU 内存）、运行日志（开关、级别、保留时长、查看目录）、关机/重启时自动调整音量、更新检测（启用/测试版/手动检测）
- **快捷键设置**：打开设备信息/音量控制快捷键、音量控制快捷键（提高/降低/静音）、输出设备快捷键集中管理（在音量控制页为设备设置的快捷键，即使设备断开也保留，支持修改、清除、删除）
- **设备信息设置**：设备过滤（正则表达式编辑、设备去重）、显示无名称蓝牙设备、使用系统蓝牙连接、2.4G 设备列表、设备分组管理
- **音量控制设置**：音量控制页设备列表独立显隐控制、关机/重启时自动调整音量

### 更新检测

- 启动时自动检测 GitHub 新版本（延迟 3 秒，非 autostart 模式）
- 有新版本时 toast 通知，点击跳转下载页
- 支持检测测试版（Pre-release）开关
- 手动检测按钮，实时反馈结果
- 使用 semver 语义化版本比较

### 其他

- 单实例模式，重复启动时自动聚焦已有窗口
- 设置页窗口状态自动记忆
- 关于页面（版本信息、项目主页链接）
- 运行日志系统：支持标准/详细两级日志，可配置保留时长（一次/一天/三天/一周/一月）
- WebView2 硬件加速开关（设置页可切换，关闭后减少 GPU 内存占用）
- 音量控制全局快捷键（提高/降低/静音，通过系统媒体键模拟，显示 Windows 原生音量 OSD）
- 输出设备全局快捷键（为音频输出设备录制的快捷键，触发后快速切换默认输出设备）

## 截图

- 设备信息
  
  <img width="300" height="390" alt="设备信息" src="https://github.com/user-attachments/assets/15badf0e-34c1-480f-a5ce-c40a3ecfd65f" />
  
- 音量控制
  
  <img width="300" height="390" alt="音量控制" src="https://github.com/user-attachments/assets/a3588cf4-332f-470f-9c5b-e3b09698e5bf" />
  
- 托盘提示
  
  <img width="300" height="100" alt="托盘提示" src="https://github.com/user-attachments/assets/68e51f47-3d78-43fc-baa4-67c753301566" />

## 技术栈

| 组件 | 技术 |
|------|------|
| 框架 | Tauri v2 |
| 后端 | Rust |
| 前端 | 纯 HTML/CSS/JS |
| 设备检测 | WMI + WinRT Bluetooth + windows_pnp |
| 音量控制 | Windows Core Audio API (IAudioEndpointVolume / IAudioSessionManager2) |
| 音频事件 | IAudioEndpointVolumeCallback + IMMNotificationClient（事件驱动） |
| 2.4G 识别 | USB VID/PID 匹配（wireless_24g_devices.json） |
| BLE 电量 | GATT Battery Service (0x180F/0x2A19) |
| BTC 电量 | windows_pnp (DEVPKEY_BLUETOOTH_BATTERY) |
| 缓存 | LRU（图标缓存）、Arc（Regex/device_id 共享）、PID 缓存（图标 base64） |
| 异步 | tokio |
| 网络 | WinHTTP (windows-sys) |
| 配置 | TOML |
| WebView2 | 自定义浏览器参数（renderer-process-limit、disable-gpu、disable-breakpad） |

## 项目结构

```
PeriphMonitor/
├── libs/
│   └── windows_pnp/                  # Windows PnP 设备枚举库
├── src-tauri/
│   ├── data/
│   │   └── wireless_24g_devices.json # 2.4G 设备数据库（VID/PID → 名称/类型）
│   ├── icons/                        # 应用图标
│   ├── src/
│   │   ├── main.rs                   # 应用入口，COM 初始化，Tauri 构建
│   │   ├── state.rs                  # 全局状态（托盘位置、动画状态、自启动）
│   │   ├── config.rs                 # 配置管理（TOML 加载/保存）
│   │   ├── classify.rs               # 设备分类逻辑（PNPClass/名称匹配）
│   │   ├── device.rs                 # 数据模型（Device, DevType）与设备 ID 存储
│   │   ├── device_data.rs            # 2.4G 设备数据加载与查询
│   │   ├── dedup.rs                 # 设备去重逻辑（核心名称提取、去重插入）
│   │   ├── wmi_query.rs              # WMI 查询编排与过滤
│   │   ├── bluetooth.rs              # WinRT 蓝牙 API（配对设备、GATT/PnP 电量）+ Win32 蓝牙连接/断开
│   │   ├── audio.rs                  # 音量控制（Core Audio API）
│   │   ├── audio_notify.rs           # 音频事件监控（IAudioEndpointVolumeCallback + IMMNotificationClient）
│   │   ├── app_icon.rs               # 进程图标提取（64×64，base64 PNG）
│   │   ├── commands.rs               # Tauri 命令处理器
│   │   ├── popup.rs                  # 弹出窗口生命周期（toggle/open/close）与动画
│   │   ├── tray.rs                   # 系统托盘菜单与事件处理
│   │   ├── shortcut.rs               # 全局快捷键注册
│   │   ├── update.rs                 # GitHub 更新检测（WinHTTP + semver）
│   │   ├── windows.rs                # 窗口创建与 DWM 圆角
│   │   └── process.rs               # 进程工具（日志、ShellExecuteW）
│   ├── dist/
│   │   ├── popup.html                # 主窗口
│   │   ├── settings.html             # 设置页
│   │   ├── about.html                # 关于页
│   │   ├── scripts/
│   │   │   ├── common.js             # 共享常量与工具函数（CATEGORIES、throttle、getInvoke、右键菜单、重命名对话框、通用对话框、Toast、快捷键录制）
│   │   │   ├── popup.js              # 主窗口逻辑（设备列表、蓝牙操作、右键菜单）
│   │   │   ├── audio.js              # 音量控制逻辑（设备/会话音量、滑块、右键菜单、设备快捷键设置）
│   │   │   ├── settings.js           # 设置页入口（Tab 切换、通用设置项、saveConfig）
│   │   │   ├── settings-general.js   # 通用设置（快捷键录制、输出设备快捷键管理、开机自启等）
│   │   │   ├── settings-devices.js   # 设备信息设置（分组渲染、过滤、去重）
│   │   │   └── settings-audio.js     # 音量控制设置（音频设备列表、关机音量）
│   │   └── styles/
│   │       ├── base.css              # 基础样式（重置、滚动条、字体、音量滑块）
│   │       ├── popup.css             # 主窗口样式（设备卡片、菜单、对话框）
│   │       ├── audio.css             # 音量控制样式（设备/会话卡片）
│   │       ├── settings.css          # 设置页样式
│   │       └── about.css             # 关于页样式
│   ├── Cargo.toml
│   └── tauri.conf.json
├── .github/
│   └── workflows/                    # GitHub Actions CI/CD
├── generate_icons.mjs                # 图标生成脚本
├── package.json
└── README.md
```

### 代码组织说明

- **后端**：`dedup.rs` 封装设备去重逻辑（核心名称提取、去重插入），`wmi_query.rs` 负责 WMI 查询编排与过滤，`process.rs` 统一管理日志和 ShellExecuteW 调用，`audio.rs` 提取 `with_enumerator()` 消除 COM 初始化样板代码，`shortcut.rs` 统一注册全局快捷键，`popup.rs` 提供 `compute_position()` 计算弹窗位置，`update.rs` 实现 GitHub API 更新检测
- **前端**：按功能拆分 — `popup.js` 负责设备列表，`audio.js` 负责音量控制；共享工具函数、throttle、右键菜单、对话框、快捷键录制统一在 `common.js`；设置页按四个文件组织 — `settings.js`（入口与通用设置）、`settings-general.js`（快捷键录制、输出设备快捷键管理）、`settings-devices.js`（设备分组渲染）、`settings-audio.js`（音频设备列表、关机音量）；CSS 拆分为 `base.css`（全局重置、音量滑块）、`popup.css`、`audio.css`、`settings.css`、`about.css`
- **配置**：TOML 格式，`log_level`/`log_retention` 使用枚举类型（支持大小写不敏感反序列化），包含 hidden_devices、hidden_audio_devices、device_names、device_groups、shutdown_volume_enabled、shutdown_volume_devices、shortcut_volume_up/down/mute、device_shortcuts、default_popup_tab、hardware_acceleration 等字段
- **日志**：标准级别记录关键运行事件，详细级别记录诊断信息；支持按保留时长自动清理
- **内存优化**：图标缓存使用 LRU（容量 256）防止内存泄漏，音频回调使用 `Arc<str>` 共享 device_id 减少字符串复制，设备分类函数避免重复计算 uppercase，配置枚举替代 String 减少堆分配
- **WebView2 优化**：`--renderer-process-limit=1` 共享渲染进程，`--disable-gpu` 关闭时节省 20-40MB GPU 内存，`--disable-breakpad` 禁用崩溃转储，`--disable-features` 禁用不需要的浏览器功能
- **代码质量**：结构体可见性严格收紧（pub→pub(crate)→私有），未使用参数清理，事件监听器泄漏修复，错误处理统一（saveConfig try/catch、Promise rejection 捕获）

## 2.4G 设备支持

当前版本仅支持在项目中显示 2.4G 无线设备（按设备类型归入对应分组），**暂不支持获取 2.4G 设备的电量信息**。

### 预设设备类型

| 类型 | 分组 |
|------|------|
| `mouse` | 输入设备 |
| `keyboard` | 输入设备 |
| `audio` | 音频设备 |
| `other` | 其他设备 |

### 添加自定义 2.4G 设备

可在设置页中点击"打开"按钮，编辑 `wireless_24g_devices_user.json` 文件添加自定义设备。应用更新时不会覆盖此文件。

设备加载逻辑：先加载官方预设（`data/wireless_24g_devices.json`），再加载用户自定义文件并合并，同 VID/PID 时用户条目优先。VID 和 PID 信息可通过 [USB 设备查看器](https://www.codertools.net/tools/usb-device-viewer.php?lang=zh) 在线获取。

JSON 格式如下：

```json
{
  "VID": {
    "PID": { "name": "设备名称", "type": "mouse|keyboard|audio|other" }
  }
}
```

- `mouse` / `keyboard` → 归入"输入设备"分组
- `audio` → 归入"音频设备"分组
- `other` 或空 → 归入"其他设备"分组

### 无法获取 2.4GHz 设备电量信息

不同的 2.4GHz 设备的通信协议不同，无法做到统一获取电量信息。如需获取设备电量，需要获取设备的 VID 和 PID，然后通过 Wireshark 和 USBPcap 第三方软件嗅探设备电量发生变化时发送的数据包，并解析数据包，获取电量信息，极其复杂麻烦。

如需参考实现方案，可查看 [2.4G 无线设备电量获取项目](https://github.com/Rainbow132/2.4G-wireless-device-battery-level-acquisition)。

**欢迎有能力的开发者贡献代码或提供思路，帮助扩展对这些设备的支持。**

## 设备过滤机制

项目使用多层过滤确保只显示有意义的设备：

1. **PNPClass 白名单**：仅查询 AudioEndpoint、Bluetooth、HIDClass、Keyboard、MEDIA、Mouse、Monitor
2. **PNPDeviceID 结构过滤**：基于设备 ID 格式过滤蓝牙服务、通用 HID 接口、系统组件
3. **正则表达式过滤**：可配置的设备名称过滤规则
4. **设备去重**：按核心名称 + 连接类型去重，同名设备保留有连接类型的版本

## 构建

```bash
npm install
npm run tauri dev
```

## 下载

从 [Releases](https://github.com/oneday5799/PeriphMonitor/releases) 页面下载最新版本，支持 x64 和 ARM64 架构。

## CI/CD

推送 `v*` 格式的 tag 时自动触发 GitHub Actions 构建：

```bash
git tag v1.1.0
git push origin v1.1.0
```

工作流会自动：
- 从 tag 提取版本号并更新配置文件（tauri.conf.json、Cargo.toml、package.json、about.html）
- 并行构建 x64 和 ARM64 安装包
- 创建 GitHub Release（tag 名含 `-` 时标记为 Pre-release）

## 许可证

[MIT](LICENSE)

## 致谢

- [EarTrumpet](https://github.com/File-New-Project/EarTrumpet) — 托盘右键菜单 Windows 声音设置快捷入口的实现参考
- [BlueGauge](https://github.com/iKineticate/BlueGauge) — 蓝牙电量读取方案参考，windows_pnp 库来源
- [BluetoothAutoConnect](https://github.com/lvusyy/BluetoothAutoConnect) — 蓝牙连接/断开方案参考

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

PeriphMonitor 是一款运行在 Windows 系统托盘中的轻量级外设监控工具。通过 WMI 查询、WinRT 蓝牙 API 和 windows_pnp 库实时检测音频、USB、蓝牙、电池、显示器等外设设备，以分类列表展示状态，并提供音量控制、蓝牙连接管理、全局快捷键等功能。界面采用 WinUI 风格设计，主窗口与设置页共享统一组件与深浅主题。

## 功能

- **设备监控**：实时检测音频、USB、蓝牙、电池、显示器等设备，显示连接状态、电量与连接类型标签（蓝牙/2.4G）；支持重命名、隐藏、正则过滤、去重与自定义分组
- **蓝牙**：显示连接/配对状态与电量（BLE 走 GATT Battery Service，BTC 走 windows_pnp），支持连接/断开及跳转系统蓝牙设置
- **音量控制**：切换默认输出设备、调节音量、静音；音量精细调节开启后滑块拖动与滚轮均按 0.1 步进微调；静音锁定开关开启后，点击静音图标即锁定静音（红色图标），锁定期间拖动音量条不会改变静音状态，需再次点击图标解除；强制静音可将需多次点击才能静音的设备（如部分智能音箱）一次直接静音；按应用调节会话音量，滑块支持实时数值 tooltip；设备右键菜单支持重命名、隐藏、空间音效（关/Windows Sonic/Dolby Atmos/DTS，需系统已安装对应格式）与录制全局快捷键；支持为每个应用单独指定音频输出/输入设备（通过 SetPersistedDefaultAudioEndpoint API）
- **全局快捷键**：支持录制音量控制（提高/降低/静音）与输出设备切换快捷键；可开启共享循环切换（多个设备共用同一快捷键，按下时循环切换默认输出设备）
- **系统托盘**：左键弹出主窗口（顶部导航切换设备信息/音量控制），右键菜单提供设备信息、音量控制、音频设备切换、声音设置、开机自启、设置、关于等入口；图标悬停显示设备状态；托盘菜单中的音频设备名称支持简化显示（仅显示括号内内容）
- **设置页**：涵盖通用（开机自启、默认打开页面、硬件加速、窗口材质、日志、关机音量、更新检测）、快捷键、设备信息（过滤/去重/分组）、音量控制（音量精细调节/静音锁定/强制静音/简化设备名称/设备列表）等设置，支持深色主题与窗口背景材质（云母 Mica / 亚克力 Acrylic，Win11 22H2+ 生效，主窗口与设置页实时联动）
- **更新检测**：启动时自动检测 GitHub 新版本，toast 通知并可点击跳转下载页，支持测试版开关与手动检测

## 截图

- 设备信息

  <img width="300" alt="设备信息" src="https://github.com/user-attachments/assets/76740ebe-dd26-426e-bf6e-06ea50596c14" />

- 音量控制

  <img width="300" alt="音量控制" src="https://github.com/user-attachments/assets/d0981c19-8bdc-4383-b230-eca6a731aebd" />

- 托盘提示

  <img width="300" alt="托盘提示" src="https://github.com/user-attachments/assets/68e51f47-3d78-43fc-baa4-67c753301566" />

- 设置页面

  <img width="300" alt="托盘提示" src="https://github.com/user-attachments/assets/d2958924-e1b3-40dd-9252-b59c7c7d6ae8" />


## 技术栈

| 组件 | 技术 |
|------|------|
| 框架 | Tauri v2（Rust 后端 + 纯 HTML/CSS/JS 前端） |
| 设备检测 | WMI + WinRT Bluetooth + windows_pnp |
| 音量控制 | Windows Core Audio API（事件驱动） |
| 2.4G 识别 | USB VID/PID 匹配（wireless_24g_devices.json） |
| 电量 | BLE GATT Battery Service / BTC windows_pnp |
| 异步 / 网络 | tokio / WinHTTP |

## 项目结构

```
PeriphMonitor/
├── libs/windows_pnp/                # Windows PnP 设备枚举库
├── tools/check.mjs                  # 前端完整性守护脚本（pre-commit 调用）
├── src-tauri/
│   ├── src/                         # Rust 后端（音频、蓝牙、托盘、快捷键、材质、更新检测等）
│   ├── dist/                        # 前端（无构建，直接编辑）
│   │   ├── popup.html / settings.html
│   │   ├── styles/                  # base.css 共享基类 + popup.css + settings.css
│   │   └── scripts/                 # common.js + 双页分区脚本（命名镜像）+ 各页入口
│   ├── data/                        # 2.4G 设备数据库
│   ├── icons/                       # 应用图标
│   └── tauri.conf.json
└── .github/workflows/               # CI/CD（x64 / ARM64 构建与发布）
```

## 2.4G 设备支持

当前版本仅支持显示 2.4G 无线设备（按设备类型归入对应分组），**暂不支持获取电量**。

不同 2.4G 设备的通信协议各不相同，无法统一获取电量信息。若需实现，需先获取设备 VID/PID，再借助 Wireshark 与 USBPcap 嗅探并解析设备电量变化时发送的数据包。可参考 [2.4G 无线设备电量获取项目](https://github.com/Rainbow132/2.4G-wireless-device-battery-level-acquisition) 的实现方案。**欢迎有能力的开发者贡献代码或思路，帮助扩展对这些设备的支持。**

可在设置页点击「打开」编辑 `wireless_24g_devices_user.json` 添加自定义设备（应用更新时不会覆盖），同 VID/PID 时用户条目优先。VID/PID 可通过 [USB 设备查看器](https://www.codertools.net/tools/usb-device-viewer.php?lang=zh) 获取：

```json
{
  "VID": {
    "PID": { "name": "设备名称", "type": "mouse|keyboard|audio|other" }
  }
}
```

其中 `mouse`/`keyboard` 归入输入设备，`audio` 归入音频设备，`other` 或空归入其他设备。

## 设备过滤

1. PNPClass 白名单
2. PNPDeviceID 结构过滤
3. 可配置的正则表达式过滤
4. 设备去重（核心名称 + 连接类型）

## 构建

```bash
npm install
npm run tauri dev
```

## 开发须知

前端位于 `src-tauri/dist/`（无构建流程，直接编辑，`cargo build` 时嵌入二进制）。提交涉及 `dist/` 的改动会自动经过完整性守护脚本（引用一致性 / 未定义调用 / 语法机检 / BOM 扫描），重新克隆后请先安装钩子：

```bash
cp tools/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

也可随时手动自检：`node tools/check.mjs`。详见 [AGENTS.md](AGENTS.md)。

## 下载

从 [Releases](https://github.com/oneday5799/PeriphMonitor/releases) 页面下载最新版本，支持 x64 和 ARM64 架构。

## CI/CD

推送 `v*` 格式的 tag 时自动构建 x64 / ARM64 安装包并创建 GitHub Release（tag 名含 `-` 时标记为 Pre-release）：

```bash
git tag v1.1.0
git push origin v1.1.0
```

## 许可证

[MIT](LICENSE)

## 致谢

- [EarTrumpet](https://github.com/File-New-Project/EarTrumpet) — 托盘声音设置入口实现参考
- [BlueGauge](https://github.com/iKineticate/BlueGauge) — 蓝牙电量读取方案参考，windows_pnp 库来源
- [BluetoothAutoConnect](https://github.com/lvusyy/BluetoothAutoConnect) — 蓝牙连接/断开方案参考

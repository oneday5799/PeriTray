<p align="center">
  <img src="src-tauri/icons/128x128@2x.png" width="128" alt="PeriTray Logo">
</p>

<h1 align="center">PeriTray</h1>

> 本项目原名为 PeriphMonitor，现更名为 PeriTray。

<p align="center">
  一款轻量级的 Windows 系统托盘外设监控工具，实时显示所有连接设备的状态信息。
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-1.85+-black?style=flat-square&logo=rust" alt="Rust">
  <img src="https://img.shields.io/badge/Tauri-2.x-blue?style=flat-square&logo=tauri" alt="Tauri">
  <img src="https://img.shields.io/badge/Platform-Windows%2010%2F11%20(x64%2FARM64)-0078d4?style=flat-square&logo=windows" alt="Platform">
  <img src="https://img.shields.io/badge/License-GPL--3.0-blue?style=flat-square" alt="License">
</p>

---

## 简介

PeriTray 是一款运行在 Windows 系统托盘中的轻量级外设监控工具，实时检测音频、USB、蓝牙、电池、显示器等设备状态，并提供音量控制、蓝牙管理、全局快捷键等功能。

基于 Tauri v2 构建（Rust 后端 + 原生 HTML/CSS/JS 前端），界面采用 WinUI 风格，支持深浅主题。

## 功能

- **设备监控**
  - 实时检测音频、USB、蓝牙、电池、显示器等设备
  - 显示连接状态、电量、连接类型标签（蓝牙/2.4G）
  - 支持重命名、隐藏、正则过滤、去重、自定义分组

- **2.4G 设备电量**
  - 通过接收器厂商 HID 私有协议查询电量，后台缓存自动刷新
  - 点击刷新按钮可强制即时查询连接状态与电量
  - 支持雷蛇、罗技、飞智、狼蛛 接收器

- **蓝牙**
  - 显示连接/配对状态与电量（BLE GATT / BTC 两种路径）
  - 支持连接/断开，可跳转系统蓝牙设置

- **音量控制**
  - 切换默认输出设备、调节音量、静音
  - 音量精细调节：滑块与滚轮按 0.1 步进微调
  - 静音锁定：点击图标锁定静音，拖动音量条不改变状态
  - 强制静音：一次操作直接静音需多次点击的设备
  - 空间音效：Windows Sonic / Dolby Atmos / DTS
  - 按应用调节音量，支持为每个应用指定音频输出/输入设备
  - 设备右键菜单支持重命名、隐藏、空间音效设置

- **全局快捷键**
  - 录制音量控制（提高/降低/静音）与输出设备切换快捷键
  - 可开启共享循环切换（多设备共用快捷键循环切换）

- **系统托盘**
  - 左键弹出主窗口（设备信息 / 音量控制双页）
  - 右键菜单提供各功能入口
  - 图标悬停显示设备状态

- **设置页**
  - 通用设置、快捷键、设备信息、音量控制等分类
  - 深色主题与窗口背景材质（Mica / Acrylic，Win11 22H2+）

- **更新检测**
  - 启动时自动检测 GitHub 新版本，支持测试版开关与手动检测

## 截图

- 设备信息

  <img width="300" alt="设备信息" src="https://github.com/user-attachments/assets/76740ebe-dd26-426e-bf6e-06ea50596c14" />

- 音量控制

  <img width="300" alt="音量控制" src="https://github.com/user-attachments/assets/d0981c19-8bdc-4383-b230-eca6a731aebd" />

- 托盘提示

  <img width="300" alt="托盘提示" src="https://github.com/user-attachments/assets/68e51f47-3d78-43fc-baa4-67c753301566" />

- 设置页面

  <img width="300" alt="设置页面" src="https://github.com/user-attachments/assets/d2958924-e1b3-40dd-9252-b59c7c7d6ae8" />


## 技术栈

| 组件 | 技术 |
|------|------|
| 框架 | Tauri v2（Rust 后端 + 纯 HTML/CSS/JS 前端） |
| 设备检测 | WMI + WinRT Bluetooth + windows_pnp |
| 音量控制 | Windows Core Audio API（事件驱动） |
| 2.4G 识别 | USB VID/PID 匹配（驱动内置声明 + 用户自定义文件） |
| 电量 | BLE GATT Battery Service / BTC windows_pnp / HID Feature Report（hidapi） |
| 异步 / 网络 | tokio / WinHTTP |

## 项目结构

```
PeriTray/
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

通过接收器暴露的厂商自定义 HID 接口（Feature Report）查询设备电量，后台缓存刷新、不阻塞设备列表；点击设备信息页顶部的刷新按钮可强制现查连接状态与电量（设备休眠时可能需数秒）。

### 已支持品牌

| 品牌 | 说明 |
|------|------|
| 雷蛇 | 鼠标、键盘共 76 个 VID/PID（Orochi V2 经实机验证，其余基于同族协议移植） |
| 罗技 | Unifying / Lightspeed / Bolt / Nano 接收器（PID 范围启发识别，单下游设备时显示名称与电量） |
| 飞智 | Vader 4 Pro |
| 狼蛛 | F75 Max（仅 2.4G 接收器模式） |

完整设备清单见 [Wiki · 支持设备列表](https://github.com/oneday5799/PeriTray/wiki/11-%E6%94%AF%E6%8C%81%E8%AE%BE%E5%A4%87%E5%88%97%E8%A1%A8)。

**其中仅部分设备经实机验证，并未逐一实测**——若你的型号显示异常，欢迎提 Issue 并附 debug.log 中 `[24g]` 开头的日志行。

### 自定义设备

在设置页点击「打开」编辑 `wireless_24g_devices_user.json` 添加未收录设备（应用更新时不会覆盖），同 VID/PID 时用户条目优先。VID/PID 可通过 [USB 设备查看器](https://www.codertools.net/tools/usb-device-viewer.php?lang=zh) 获取：

```json
{
  "VID": {
    "PID": { "name": "设备名称", "type": "mouse|keyboard|audio|other" }
  }
}
```

其中 `mouse`/`keyboard` 归入输入设备，`audio` 归入音频设备，`other` 或空归入其他设备。

### 扩展支持

新品牌需在 `src-tauri/src/wireless_24g/drivers/` 下按协议族新增驱动文件（实现 `BatteryDriver` trait）。若想自行逆向其它品牌协议，可参考 [2.4G 无线设备电量获取项目](https://github.com/Rainbow132/2.4G-wireless-device-battery-level-acquisition) 的方法论。**欢迎贡献代码或思路。**

## 构建

```bash
npm install
npm run tauri dev
```

## 开发须知

前端位于 `src-tauri/dist/`（无构建流程，直接编辑）。提交涉及 `dist/` 的改动会自动经过完整性守护脚本校验。重新克隆后安装钩子：

```bash
cp tools/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

也可手动自检：`node tools/check.mjs`。详见 [AGENTS.md](AGENTS.md)。

架构、模块详解与开发指南见 [项目 Wiki](https://github.com/oneday5799/PeriTray/wiki)。

## 下载

从 [Releases](https://github.com/oneday5799/PeriTray/releases) 页面下载最新版本，支持 x64 和 ARM64 架构。

## CI/CD

推送 `v*` 格式的 tag 时自动构建 x64 / ARM64 安装包并创建 GitHub Release（tag 名含 `-` 时标记为 Pre-release）：

```bash
git tag v1.1.0 && git push origin v1.1.0
```

## 许可证

[GPL-3.0 LICENSE](LICENSE)

## 致谢

- [BluetoothAutoConnect](https://github.com/lvusyy/BluetoothAutoConnect) — 蓝牙连接方案参考
- [BlueGauge](https://github.com/iKineticate/BlueGauge) — 蓝牙电量方案参考，windows_pnp 来源
- [EarTrumpet](https://github.com/File-New-Project/EarTrumpet) — 托盘音量入口参考
- [win11React](https://github.com/blueedgetechno/win11React) — WinUI 样式参考
- [WinUIonWeb](https://github.com/Furry-Xiyi/WinUIonWeb) — WinUI 样式参考
- [2.4G-wireless-device-battery](https://github.com/Rainbow132/2.4G-wireless-device-battery-level-acquisition) — 2.4G 私有协议逆向方法论
- [OpenRazer](https://github.com/openrazer/openrazer) — 雷蛇私有协议参考
- [Solaar](https://github.com/pwr-Solaar/Solaar) — 罗技电量读取路径参考
- [OpenLogi](https://github.com/AprilNEA/OpenLogi) — 罗技 HID++ Rust 参考实现

## 支持

如果觉得本项目的确对你有帮助，欢迎支持本项目。

<img width="240" src="https://github.com/user-attachments/assets/9cf3808b-5239-498f-99bc-2e5f975f0729" />
<img width="240" src="https://github.com/user-attachments/assets/bc40b2cb-97a1-43ba-a268-f2587d74ae39" />

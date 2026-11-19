/// PeriTray 构建脚本。
///
/// ⛔⛔ 这里**不是**简单一句 `tauri_build::build()`，而是显式注入自定义应用程序清单
/// （`app.manifest`）—— 因为 B3 任务栏 widget 需要
/// 「`WS_EX_LAYERED` 用在**子窗口**上」（Win8+ 能力），而 Windows 靠清单里的
/// `<compatibility><supportedOS>` 判定应用面向的 OS 代际。Tauri 2 的**默认清单只有
/// Common-Controls、没有 supportedOS**（`tauri-build-*/src/windows-app-manifest.xml`）
/// ⇒ 对「创建时直接带 `WS_CHILD + WS_EX_LAYERED`」形态，建窗**恒失败且 err=0**
/// （极易误判为代码写错）。⚠️ 本仓实际走的是 **popup → `SetParent`** 路线，它**不需要**
/// 该清单；此处补清单属**基础设施改进**（让直建子窗的形态将来可用）。
///
/// ⚠️ `app_manifest(..)` 是**整份替换**而非合并 ⇒ `app.manifest` 里已**逐字保留**
///    默认清单的 Common-Controls v6 `dependency`（否则 Tauri 对话框类 API 会失效）。
///
/// ⛔⛔⛔ **`app.manifest` 绝对不能带 XML 声明（`<?xml ... ?>`）与注释**（2026-09-24 实测）：
///   带上 `<?xml version="1.0" encoding="UTF-8" standalone="yes"?>` 时，
///   进程**根本无法启动**，`Start-Process` 报：
///     「应用程序无法启动，因为应用程序的并行配置不正确」
///   ⭐ 取证（最直接的一条）：事件日志 **SideBySide / 事件 ID 59**，内容为
///     「…激活上下文生成失败。在指令清单或策略文件…**第 1 行**出现错误。**无效的 Xml 语法**。」
///     查询：`Get-WinEvent -FilterHashtable @{LogName='Application'; StartTime=...}`
///           再筛 `Message -match '<exe 名>'`。
///   ⇒ 第 1 行就是那句 XML 声明；去掉声明（与注释）后**立即可启动**。
///   ⭐ 二分法实测：默认清单 ✅ / 自定义+声明 ❌ / 自定义无声明无注释 ✅
///   ⚠️ 因此**所有说明性文字只能放在本文件（Rust 注释里）**，不能放进 manifest。
///
/// 调用链（源码实证）：`try_build` → `Attributes::windows_attributes` →
/// `WindowsAttributes::app_manifest` → `tauri-winres` 生成 `.rc` 的
/// `<FILETYPE> 24 { ... }`（资源类型 24 = `RT_MANIFEST`、名称 ID 1）。
fn main() {
    let attrs = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new().app_manifest(include_str!("app.manifest")),
    );
    tauri_build::try_build(attrs).expect("tauri-build 失败（自定义清单注入或其它构建期检查）");
}

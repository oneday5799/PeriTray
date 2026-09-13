## ✨ 新功能

- 新增 MSIX 打包支持，为后续 Microsoft Store 发布做准备

## 🐛 问题修复

- 修复弹窗中滚动条出现或消失时卡片宽度来回抖动的问题
- 修复 WebView2 COM 调用中 controller 引用泄漏（背景色重试/失焦/开关等场景累计）

## 🧹 内部优化

- 日志调用迁移为惰性求值宏，日志关闭时零格式化开销与零堆分配
- WebView2 COM 调用全面类型化，消除手算 vtable 槽位与 transmute
- 重新引入弹窗隐藏时 WebView2 LOW 内存档位
- 为音频会话枚举链路添加诊断日志，便于排查偶发会话列表为空的问题
- CI 工作流：beta 版跳过 MSIX 构建并传递 Publisher DN secret

**完整变更列表**：https://github.com/oneday5799/PeriTray/compare/v1.3.5...v1.3.6
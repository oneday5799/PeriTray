# AGENTS.md

## Commit 规范

### 标题格式

```
<type>(<scope>): <中文一句话概括>
```

- **type**（九选一，与仓库既有用法一致）：
  - `feat` 新功能　`fix` 缺陷修复　`refactor` 重构（无行为变化）
  - `perf` 性能优化　`style` 界面样式调整　`docs` 文档
  - `ci` 构建/发布流程　`chore` 杂项（依赖/工具链/版本号等）　`revert` 回退
- **scope** 可选，标注影响域：`frontend` / `core` / `popup` / `tools` / `release` 等；
  影响面广或跨域时省略
- 概括用中文，一行说清"做了什么"；过细的改动说明放 body，不放标题

### Body

- 写清「为什么改 + 怎么改的 + 影响边界」；有根因的必须写根因
- **commit body 是 Release Notes 的信息源，宁详勿略**
- 无行为变化的重构需显式声明「纯等价重构 / 行为零变化」；有回归风险的列出回归点

### 其他

- 发版的版本号 bump 单独成提交：`chore(release): vX.Y.Z`
- 涉及 `src-tauri/dist/` 的提交会被 pre-commit 钩子自动校验（见下节）
- **Rust 改动提交前必须 `cargo check` 零警告**（main.rs 有 `#![warn(unused_imports, dead_code)]`，
  出现 warning 即视为未完成）；无自动闸门，靠自觉执行

## Release Notes 风格规范（每次发版必循）

面向普通用户写作：只写用户可感知的结果，不写实现机制。

### 分节（按实际内容取用）

```markdown
## ✨ 新功能
## 🐛 问题修复
## 🧹 内部优化        ← 重构/清理/性能等一切用户无感知的变化归此节
```

### 条目写法

- 一条一句话，动词开头直给结果：「修复…的问题」「新增…」「不再…」
- **禁止实现术语**：API 名、函数名、commit 号、「架构/波段/接口层」类词汇一律不出现；
  必要的产品名词保留（空间音效、2.4G、快捷键等）
- 关键限定必须保留在条目内：实验性功能、默认关闭、需重启生效等
- 性能类用户可感知的（如"内存占用降低"）可入 🧹 或单列 ⚡ 节

### 结构约定

- 节内条目按用户影响程度排序（重要在前）
- 条目末尾以 `**完整变更列表**：<compare 链接>` 收尾
- beta 测试版注明承接关系（如「包含自上一测试版以来的全部改进」）
- 纯晋级发布（tag 与前一 tag 无代码差异）写简短宣告 + 主要能力回顾
- 首个版本无 compare 链接，写功能总览
- 信息源取自本版全部 commits 的 body

### 发布流程

1. 版本号同步五处：tauri.conf.json、Cargo.toml `[package]`、package.json、
   Cargo.lock（`cargo check` 自动刷新）、settings.html 占位文案——
   单独 `chore(release)` 提交并 push
2. notes 写入临时文件经 `--notes-file` 传入（避免 shell 转义问题）
3. 创建发布：
   ```bash
   gh release create v<ver> --target <完整SHA> --title "PeriphMonitor v<ver>" \
     --notes-file <notes文件> --latest
   ```
   （--target 必须传完整 SHA，短 SHA 会 422）；tag 含 `-`（如 v1.2.9-beta.1）
   时 CI 自动标记为预发布；CI 构建后自动向该 Release 追加安装包产物
4. CI 使用 softprops/action-gh-release@v3 + generate_release_notes，
   对已存在的 Release 是更新追加而非报错，手工先建 Release 不冲突

## 代码与注释风格

- **字符串引号**：JS 统一双引号；字符串内容本身含双引号时允许单引号包裹（免转义）
- **缩进**：JS / CSS 两空格，Rust 四空格，一律空格禁 Tab
- **命名**：JS 函数/变量 camelCase、CSS 类名 kebab-case（变体用 `--` 后缀）、
  Rust 与配置键 snake_case
- **异步**：以 async/await 为主；fire-and-forget 场景可用 `.then().catch()` 链
- **注释语言**：一律中文；专有名词 / 算法名 / 标准名可保留英文原文（如 WinRT、COM、牛顿迭代）
- **分区样式**：`// ── 分区名 ──…` 长横线补齐对齐，Rust 与 JS 同款
- Rust 用 `///` 为 pub 项写文档注释；日志统一走 `process::append_log` 并带 `[模块]` 前缀
  （[popup] [tray] [audio] [bt] [update] 等，新增模块先定标签）
- JS 文件头注释四要素见「前端架构备忘」

## 前端完整性守护（强制）

所有涉及 `src-tauri/dist/` 的改动，提交时会自动经过守护脚本检查
（`.git/hooks/pre-commit` → `node tools/check.mjs`，每次提交全量运行，<1s）。

**四类校验**：
1. HTML 引用与磁盘文件双向一致（含孤立文件检测）
2. 跨文件调用审计：调用的标识符必有声明
3. 全量 JS `node --check` 语法机检
4. BOM 扫描（CSS/JS/HTML 禁止 UTF-8 BOM）

**防护边界**：结构完整性闸门。能拦引用缺失/孤立文件/未定义调用/语法错误/BOM；
拦不住 CSS 语义错误、合法语法下的逻辑 bug、运行时行为问题——这些仍需构建后人工回归。

**例外通道**：
- `git commit --no-verify` 可跳过钩子，仅限明知未完成的 WIP 中间提交
- 提交前可随时手动自检：`node tools/check.mjs`

**钩子重装**（`.git/hooks/` 不随仓库走，重新克隆后执行）：

```bash
cp tools/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

**维护注意**：新增页面/目录需同步更新 `tools/check.mjs` 的 PAGES 数组；
若标识符审计出现误报，优先扩展 check.mjs 的声明提取规则，而非绕过钩子。

## 前端架构备忘

- 结构：popup/settings 双页体系，脚本"分区在前、入口最后"，命名镜像
  （`popup-{devices,audio}.js ↔ settings-{devices,audio}.js`），全部 JS 带标准头注释
  （四要素：文件职责 / 加载序 N/N · 提供：… / 依赖：…）
- **invoke 双轨是有意设计，勿"统一"**：popup 页经 common.js 的 `getInvoke()`
  防御式获取（弹窗生命周期内 webview 注入时序敏感）；settings 页依赖 common.js
  顶层的 `const { invoke } = window.__TAURI__.core` 全局词法绑定裸用——
  重排加载序或迁移文件时须保持各自语义
- 已否决路线：方案乙 ESM 迁移（触发重启条件：前端规模翻倍 / 多人协作 /
  config 共享实际出 bug；届时可先考虑 config 抽为经典脚本单例的廉价中间路线）
- 材质系统收敛（删除 settings-general 回调手动三件套）暂缓，
  下次因其他原因动材质代码时顺手做并实测闪烁
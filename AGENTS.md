# AGENTS.md

本文件按「提交 / 写码 / 发版 / 质量 / 验收 / 架构 / 维护」组织开发约定，改动涉及对应环节时先查相关节。

> **本文件只放「必须随身带的强制约定」**；原理解释、证据与完整复盘在 Wiki：
> [13-架构决策与纪律](https://github.com/oneday5799/PeriTray/wiki/13-架构决策与纪律)
> （每条纪律的「为什么 + 反例 + 判据」）、
> [14-工程实践与验收方法论](https://github.com/oneday5799/PeriTray/wiki/14-工程实践与验收方法论)
> （可证伪验收、注入式验证、避坑清单 30 条）、
> [12-代码审查与整改复盘](https://github.com/oneday5799/PeriTray/wiki/12-代码审查与整改复盘)
> （审查结论的账）。**与 Wiki 冲突时以本文件为准。**

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
- **Rust 改动提交前必须过 `cargo fmt --check`、`cargo check` 零警告、`cargo clippy`**
  （main.rs 有 `#![warn(unused_imports, dead_code)]`）；由 pre-commit 钩子自动执行——
  Rust 文件有暂存改动时增量运行（`fmt` + `check` 热增量约 3s，clippy 另加约 6~14s），
  格式不符、有 warning 或 clippy 未通过即拦截

## 代码与注释风格

- **字符串引号**：JS 统一双引号；字符串内容本身含双引号时允许单引号包裹（免转义）
- **缩进**：JS / CSS 两空格，Rust 四空格，一律空格禁 Tab
- **命名**：JS 函数/变量 camelCase、CSS 类名 kebab-case（变体用 `--` 后缀）、
  Rust 与配置键 snake_case
- **RAII 守卫的绑定命名**：**需要被 `move` 闭包捕获的守卫（外层绑定）不得以 `_` 开头**——
  下划线会同时消掉 `unused_variables` 告警，而「未被引用」正是闭包捕获失败的直接原因；
  仅在被移入目标作用域后、只承担 Drop 职责的**内层绑定**，`_` 前缀才是正确用法
  （如 `let _guard = guard;`）。**这条规则的作用域是「外层守卫」，不是「所有 RAII 守卫」**——
  对 `let _guard = crate::state::lock_unpoisoned(&LOCK);` 这类内层 Drop-only 绑定，
  去掉下划线只会制造永久误报（P3-14）。
  ⚠️ **别把这条读成「护栏已闭合」**：`unused_variables` 对下划线绑定**主动豁免**，
  「守卫是否被持有」在源码上**不可判定**，而「是否遵守命名约定」**没有任何机械检查**——
  这正是 P0-1 进入仓库的路径。可靠的只有**结构约束**（见下一条）。详见 Wiki 13 §3
- **单飞守卫一律复用 `state::SingleFlightGuard`，不新建类型**；**占用点固定在函数顶部**
  （不得推迟到 `std::thread::spawn` 那一行，否则 `resume_webview` / `set_size` 等窗口操作
  会在**无守卫状态下**执行）；需要「唯一出口」时把守卫做成**必需参数**
  （如 `fn spawn_animation<F>(guard: SingleFlightGuard<'static>, f: F)`）——
  漏传即 `error[E0061]`，是**类型级拦截，不是 lint 提醒**。
  **评审检查项**：见到 `ForceGuard` / `AnimatingGuard` 这类**语义相同的新守卫类型**即为违规。详见 Wiki 13 §3
- **失败路径不得留下成功状态（P0-2 / P3-12 / P2-12）**：凡「先置位 / 先记录状态、
  后续步骤可能失败」的代码，**必须在失败分支显式回滚**，或用 RAII 守卫保证作用域退出即复位；
  **注册 / 注销类状态变更只在成功后才更新状态集合**（否则一次失败 = 永久跳过重试）。
  **评审检查项**：对每处 `compare_exchange` / `store(true)` / `registered.insert` 问一遍
  **「这个置位有回滚吗？」**。⚠️ 三条的机制**并不同族**，不要强行抽共用代码产物——
  共用的是**规范条目 + 评审检查项**。详见 Wiki 13 §4
- **`if let` 的临时量存活至整个块尾（edition 2021）**：
  `if let Some(x) = lock_unpoisoned(..).take() { com_call(&x); }` 会让 COM / I/O
  落在**持锁区内**。先把取值写成独立语句，再 `if let`。
  （对照：`if !lock(..).insert(..)` 是 `if` 而非 `if let`，条件求值结束即释放，**不受影响**。）详见 Wiki 13 §1
- **全局标志必须有与构建 profile 无关的兜底**：release 是 `panic = "abort"` ⇒
  **栈不展开、`Drop` 不执行**，「靠 Drop 在展开时复位」的收益**在 release 下一律为空**。
  凡「永久失效」场景必须有**单调时钟超时自愈**
  （范式：`ANIMATION_STARTED: AtomicU64` + 各入口判据改「已占用**且未超时**」）。详见 Wiki 13 §10
- **异步**：以 async/await 为主；fire-and-forget 场景可用 `.then().catch()` 链
- **异步上下文禁止 `std::thread::sleep`（P2-10）**：`tauri::async_runtime::spawn` 的
  async 块运行在 tokio 工作线程上，阻塞式 sleep 会**占死一个执行器线程**
  （worker 数 ≈ CPU 核数，几处并发就足以让整条异步链路停摆）。一律改
  `tokio::time::sleep(dur).await`（`tokio` 已启用 `time` feature）。
  **反向同样要注意**：`tokio::time::sleep` 在没有运行时上下文的地方（`std::thread::spawn`
  的真线程、同步函数体、`#[test]`）调用会 **panic**，那里用 `thread::sleep` 才是对的。
  **评审检查项**：`async fn` / `async move { }` 块内出现 `thread::sleep` 即为违规；
  反之在真线程里出现 `tokio::time::sleep` 也是违规。静态闸门覆盖不到这条
  （见「防护边界」），只能靠评审
- **`app.listen` 的回调没有专属线程，它跑在 `emit` 的调用线程上**：Tauri **不设**
  「事件线程」——`emit` 在**调用它的那个线程**上同步逐个执行回调
  （`tauri/src/event/listener.rs:204`，且此时还持着 `handlers` 锁）。因此「回调里阻塞
  会不会卡 UI」**取决于谁 emit，不能一概而论**：本仓托盘菜单/托盘图标事件由 tao 主循环派发
  （`tauri/src/app.rs` 的 `EventLoopMessage::MenuEvent` / `TrayIconEvent` 分支），
  同步 `#[tauri::command]` 也在调用线程上执行 ⇒ **这两条路径的回调就在主线程上**，
  在其中做 COM 枚举、设备/菜单重建、`Command::output()` 等待等耗时操作，
  会直接表现为**窗口点不动、托盘无响应**（最短触发路径往往是托盘菜单单击）；
  而 `audio_notify` 的音频 STA 线程 emit 时，阻塞只伤那条后台线程。
  ⇒ 纪律不因「也许不在主线程」而放宽：回调内只保留必须即时反映的轻量动作
  （读配置、更新原子标志、改勾选文案），其余一律 `std::thread::spawn` 下放。
  **评审检查项**：每新增一个 `app.listen`，逐行确认回调体内没有跨进程等待、
  没有设备枚举、没有整棵菜单/图标的构造；同理，`Mutex`/`OnceLock` 的持锁区内
  不得调用上述耗时函数（先取句柄 → 锁外构造 → 短暂持锁替换）
- **持锁区不得调用「同步等主线程」的 API（P0，会永久死锁）**：Tauri 的菜单/托盘 API
  （`MenuItem::with_id`、`Submenu::append`、`set_text`、`set_menu`、`set_icon`、`set_tooltip` …）
  与窗口 getter（`is_visible`、`hwnd`、`show` …）内部都经 `run_item_main_thread!` 展开为
  `run_on_main_thread(..)` + `rx.recv()`——**无超时地同步等待主线程**。而主线程自身会通过
  `config::with_config(_mut)` 读配置（`set_window_material`、`get_config`、快捷键分发、
  tooltip 刷新…）。因此「子线程持配置锁/托盘锁 → 调菜单 API」与「主线程 → 等该锁」
  构成 **AB/BA 死锁：永久冻结，看门狗也救不回**（其探活 `is_visible()` 同样要主线程），
  只能被系统按「应用无响应」终止（退出码 `0xCFFFFFFF`）。
  **正确写法**：锁内只取纯数据快照或句柄克隆（`TrayIcon`/`MenuItem` 均 `Clone`），
  释放锁后再调用 API——参考 `tray::build_audio_devices_menu` / `update_audio_devices_menu`。
  **评审检查项**：任何 `with_config(_mut)`、`lock_unpoisoned(..)`、`.lock()` 的持锁区内，
  逐行确认没有 Tauri 菜单/窗口 API、没有 `run_on_main_thread`、没有 COM/WMI 与文件 I/O。
  ⚠️ 顺带：**`.lock()` 本身就不该出现**——加锁一律走统一入口，`state.rs` 的
  `lock_unpoisoned` 实现与其中毒单测除外（P3-10 收敛后**已无例外**）。判据见
  `state.rs` 模块文档 §五③。
  **其中「菜单/托盘 API」这一半已有机械防线（B8）**：这类调用一律走 `tray.rs` 的薄包装
  （`apply_tooltip` / `apply_text` / `apply_icon` / `apply_menu`），包装内的
  `debug_assert!(!config::config_lock_held())` 会在**开发期立刻 panic** 并指出是哪个 API。
  ⚠️ **新增托盘/菜单 setter 调用必须走包装，不要直接调** —— 直接调会绕过断言。
  其余几类（窗口 getter、COM/WMI、文件 I/O）仍只能靠评审
- **锁序登记在 `state.rs` 模块文档（P3-10）**：全局锁的**层级**、**允许的嵌套边白名单**
  （当前仅 3 条：`DEVICES_CACHE→CONFIG`、`PERSIST_LOCK→LAST_CONFIG_CONTENT`、
  `BT_LOCK→BLE_CONN`）、**禁止的反向边**（一旦出现即构成 AB/BA 死锁条件）、
  以及自查命令，全部集中在 `src-tauri/src/state.rs` 文件头的模块文档里。
  新增锁、或新增任何「持 A 取 B」之前**先查那张表**——**不在白名单里的边一律按缺陷处理**。
  为什么单列这一条：AB/BA 死锁既不报编译错、也不产生 panic 栈，只表现为进程静默僵死
  （日志停在同一行、窗口点不动），散落在各文件的行内注释挡不住新调用点。
  与「防护边界」同理，此处只做指针，**表本身以 `state.rs` 为单一来源**，勿在此重复列举。
- **注释语言**：一律中文；专有名词 / 算法名 / 标准名可保留英文原文（如 WinRT、COM、牛顿迭代）
- **分区样式**：`// ── 分区名 ──…` 长横线补齐对齐，Rust 与 JS 同款
- **Rust 文档注释与日志**：`///` 用于 pub 项；日志统一走 `process::append_log`（标准级）
  / `process::append_verbose_log`（详细级）并带 `[模块]` 前缀（[popup] [tray] [audio]
  [audio_notify] [bt] [update] [material] [event] [heartbeat] [watchdog] [window] 等，
  新增模块先定标签）
- **忽略 `Result` / `let _ =` 必须写明「为什么安全」（P3-7）**：`let _ = f()` 会静默吞掉
  失败，是「点了没反应」类哑故障的常见来源。允许忽略，但**必须**在紧邻注释里说明属于哪一类：
  ① **无状态后果**——UI 呈现类（`set_size` / `set_position` / `set_focus` / `set_text` /
  `set_icon` / `set_tooltip` / `emit`）：失败只是这一次画面没刷新，下次刷新即自纠；
  ② **有下游兜底**——如 `create_dir_all` 失败会让随后的写入也失败，而那个失败会报错；
  ③ **不可上报**——托盘菜单事件没有调用方，只能记日志。
  反之，凡是「失败后**状态/配置与用户操作不一致**」的（切默认音频设备、写配置文件、
  注册快捷键…）**一律不得静默**：能返回 `Err` 就返回，不能就记**标准级**日志。
  ⚠️ **不要机械地把全仓 `let _ =` 都改掉**：本轮复核全仓 128 处，真正需要提升的只有个位数，
  其余属①/②类，改了只是噪声。
- **JS 头注释**：四要素（文件职责 / 加载序 N/N · 提供：… / 依赖：…）见「前端架构备忘」

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

**走「打 tag → CI 自动发布」，不需要手工 `gh release create`。**

1. **版本号同步五处**：tauri.conf.json、Cargo.toml `[package]`、package.json、
   Cargo.lock（`cargo check` 自动刷新）、settings.html 占位文案；
   **并把 release notes 写进仓库根的 `tmp-release-notes.md`**
   （面向用户写作，规范见上节；该文件随发版提交一起入库）
2. 两者一起作为**单个** `chore(release): vX.Y.Z` 提交并 push
3. **打 tag 并 push**：`git tag v<ver> && git push origin v<ver>`
   ——tag 含 `-`（如 v1.3.7-beta.1）时 CI 自动标记为预发布
4. **CI 自动发布**：release job 依次做 checkout → 解析 `tmp-release-notes.md` →
   双矩阵构建 NSIS 安装包 → `softprops/action-gh-release@v3` 创建 Release 并追加产物。
   正文规则：**有 `tmp-release-notes.md` 就用 `body_path` 注入它、并关闭 `generate_release_notes`**
   （否则自动生成的 changelog 会拼在手写正文后面）；**没有则回退**到 `generate_release_notes`。
   ⚠️ 本仓**不用 PR** ⇒ 自动生成的正文实际只有一行 compare 链接，
   **手写 notes 才是正文的主要来源**。
   ⚠️ 该 action 对「空 `body_path`」与「文件不存在」**都会静默回退**，不会让发布失败。
5. **发版后删除 `tmp-release-notes.md`**，单独一个提交
   （`chore: 移除临时 release notes 文件`）——保持工作区干净，避免下一版误用旧 notes
6. **WIKI 校准轮**（正式版必做，beta 跳过；连续多个 beta 晋级时补做一次）：
   版本演进史补行 / 进行中分支表刷新 / 本版 commits 是否有漏更的触发项 /
   README↔WIKI 入口互通——清单见 WIKI「Wiki-维护规范」§2 模式 B

## 提交自动闸门（强制）

每次提交全量运行 `.git/hooks/pre-commit` → `node tools/check.mjs`（<1s）；
Rust 文件有暂存改动时增量追加 `cargo fmt --check` + `cargo check` 零警告校验
（约 3s），再追加 `node tools/check-clippy.mjs --optional`（约 6~14s，
**未装 clippy 组件时告警跳过**——CI 上同一条命令**不带** `--optional`，是硬失败）。

**七类校验**：
1. HTML 引用与磁盘文件双向一致（含孤立文件检测）
2. 跨文件调用审计：调用的标识符必有声明
3. 跨文件同名全局函数检测：经典脚本后加载会遮蔽先加载（防 updateDeviceCard 类覆盖回归）
4. 全量 JS `node --check` 语法机检
5. BOM 扫描（CSS/JS/HTML 禁止 UTF-8 BOM）
6. 版本号一致性：tauri.conf.json / Cargo.toml / Cargo.lock / package.json /
   settings.html 占位 五处须为同一版本（防发版间隙漂移）
7. Toast 契约（P1-6 的两半，必须成对）：`showToast` 实参不含 HTML 标签 +
   `.toast` 的层叠 `white-space` 为 `pre-line`

**Rust 侧另有 `cargo clippy` 闸门**（`node tools/check-clippy.mjs`，CI 与 pre-commit 共用
同一条命令，**单一来源在该脚本**）。它不是裸 `-D warnings`，而是
**`-D warnings` + 一份存量基线 `-A`（条数见脚本）+ 3 条显式开启**：

- **为什么带基线**：2026-09-18 实测当前 HEAD 默认集报 **88 条**（bin 44 个唯一位置 + test
  单元重复计数），全是风格类（`redundant_closure` 16 / `field_reassign_with_default` 16 /
  `manual_clamp` 8 …），**与本次审查的 40 条发现零交集** ⇒ 裸 `-D warnings` 只会让 CI
  首次即红，逼人做无收益的风格改动。故存量按 lint 粒度封存，
  **修掉一条就从基线删一条**（基线即待办）。
  ⚠️ **别在任何文档里写基线的条数**——它随修复递减，写死必然漂移。
  **条数以 `tools/check-clippy.mjs` 的 `BASELINE_ALLOW` 为准**（该脚本的运行期消息
  也从数组长度派生，不是硬编码）——**本文件有意不写条数**，与上一条同理。
- **3 条显式开启**（实测当前 0 命中，直接对应「持锁区只能做纯内存操作」）：
  `clippy::await_holding_lock` / `clippy::await_holding_refcell_ref` / `clippy::mutex_atomic`。
- **边界（勿读成「Rust 侧已闭合」）**：基线里那些 lint**对新增代码同样放行**；
  `.lock().unwrap()` **clippy 默认不覆盖**（`unwrap_used` 属 restriction 组、默认关闭，
  开启后全仓 94 条）；`let _ =` 丢弃 must_use **无机械防线**
  （`SingleFlightGuard` 没有 `#[must_use]`，而 `let _x = guard` 被语言规范主动豁免）；
  异步上下文里的 `thread::sleep`（全仓 24 处）**无对应 lint**。
  基线里 lint 名写错**不会静默失效**（`-D warnings` 把 `unknown_lints` 升级为 `E0602`），
  但**改基线必须实跑一次**。**详见 `tools/check-clippy.mjs` 头部注释。**

**Rust 工具链版本是固定的（2026-09-19）**：由仓库根的 `rust-toolchain.toml` 指定
（当前 `1.98.1`）。rustup 对仓库内**任何** `cargo` / `rustc` / `rustfmt` / `clippy` 调用
都会读它，优先级 `RUSTUP_TOOLCHAIN` 环境变量 > `rust-toolchain.toml` > `rustup default`
⇒ **本地与 CI 同版，且不需要改本机 `rustup default`**。`ci.yml` 与 `release.yml` 都
**从该文件读 channel 再安装**，并在同一步断言「生效工具链 == 文件里的值」——
若被环境变量之类覆盖，那一步当场变红，而不是让两边静默跑在不同版本上
（`tools/check-clippy.mjs` 在本地也有同款**只告警**自检）。
⚠️ **改 channel 必须自己先把五道闸门跑一遍**：新版本往 `clippy::all` 加 lint 会命中
**存量**代码，在 `-D warnings` 下直接变编译错误——2026-09-19 的
`clippy::chunks_exact_to_as_chunks`（命中 `app_icon.rs`）正是此例，当时本地 `stable`
是 1.96 而 CI 已到 1.98，于是「本地绿、CI 红」。升级三步写在 `rust-toolchain.toml` 文件头。

**文档侧另有 `node tools/doc-table-audit.mjs`**（**CI 接入，pre-commit 不接**——
文档变更频率低，且 pre-commit 要保持既有的 <1s + 3s + 14s 预算）。
**默认体检目标是主仓内的交付稿 `AGENTS.md` 与 `README.md`**
（也可显式传文件 / 目录；⚠️ **不扫** `tools/doc-table-audit.selftest.md`——那是故意损坏的夹具）。
它做两件事：**表格列数体检**（同一张表内列数必须一致）与**自指型断言体检**
（交付稿不得残留 `§N.x` 这类**活指针**）。四条判据细节（都是实测踩出来的，**改脚本前先读**）：

- **列数判据必须是「同一张表内一致」**，不能写成「所有编号行都是 N 列」——后者会把文档里
  本就存在的 5/6/7 列表全判成异常（首次跑产出 **79 行假阳性**）。
- 必须**排除行内代码里的转义竖线** `\|`（GFM 下表内解析先于行内代码，**裸 `|` 会渲染错位**）。
- **活指针正则必须覆盖「任意 `§N.x`」**，不能只写 `§2.x` / `§x.x`——后者会漏掉 `§6.x`，
  而**手工体检正是因此漏报**（它当时自称「0 处活指针」）。
- **必须区分「使用」与「提及」**：`§N.x` 落在 `「…」` 内是**引用旧文本**，不计硬失败
  （说明某个**已修好**的断指针时会这么写）；首次跑出 16 处，
  逐条看过确认 **15 处属此类、仅 1 处是真裸引用**。⇒ 判据不收窄则假阳性率 15/16。

**判据自身的承重性也有机械防线**：`tools/doc-table-audit.selftest.md` 是**故意损坏**的夹具，
`tools/verify-final.sh` 断言它「**退出码 1 且恰好报 4 个问题**」——少于 4 即某条判据已失效。
⚠️ **边界**：本工具只能做「指针存在吗」与「裸占位符」的机械检出；
**后两级（「指对」+「内容相符」）只能人读**，脚本一律归入 INFO 提示。

**防护边界**：结构完整性闸门。能拦引用缺失/孤立文件/未定义调用/同名全局函数覆盖/
语法错误/BOM/版本漂移/Rust 格式不符/编译警告；拦不住下面这几类，改动后**必须人工回归**：
CSS 语义（属性值/选择器/层叠覆盖）、其余合法语法下的逻辑 bug、运行时行为问题
（事件时序）、**跨文件加载序**（第 2 类审计的声明池**按页汇总且无序**——实测把
`common.js` 排到最后时闸门仍报通过，而运行时报
`ReferenceError: registerContextMenu is not defined`，详见 Wiki 04 §3.1）、
**Rust 侧的一切**（锁纪律、异步上下文阻塞 sleep、锁序死锁）、
未加守卫的 API 访问与未 catch 的 Promise。**同类清单另见 `tools/check.mjs` 头部注释
（P3-8），两处是同一份边界的两种表述，改一处请同步另一处。**

**例外通道**：
- `git commit --no-verify` 可跳过钩子，仅限明知未完成的 WIP 中间提交
- 提交前可随时手动自检：`node tools/check.mjs`

**钩子重装**（`.git/hooks/` 不随仓库走，重新克隆后执行）：

```bash
cp tools/pre-commit .git/hooks/pre-commit && chmod +x .git/hooks/pre-commit
```

**维护注意**：新增页面/目录需同步更新 `tools/check.mjs` 的 PAGES 数组；
若标识符审计出现误报，优先扩展 check.mjs 的声明提取规则，而非绕过钩子。

## 验收与回归纪律（修复类改动强制）

> 原理解释、实测证据与完整避坑清单（30 条）见
> Wiki [14-工程实践与验收方法论](https://github.com/oneday5799/PeriTray/wiki/14-工程实践与验收方法论)。
> 本节只列「必须照做」的部分。

- **四道闸门是「回归闸门」，不是「验收闸门」**——它们**只确认没改坏，不确认改对了**。
  实测证据：`cargo test` 与审查发现的 15 个文件**零交集**（那些文件全部 0 个 `#[test]`），
  它在**修复之前就已经全绿** ⇒ 「通过」只意味着编译过、格式对、没碰坏前端。
- **每批完成 = 回归段 + 验收段，缺一不可**：① 四道闸门全绿；
  ② **本批每条缺陷各自的「可证伪验证」逐条通过**。
  **没有第 ② 段，这一批的「完成」是自证的。**
- **判据必须可证伪**：一条验证有效，当且仅当**在未修复的代码上它会失败（或给出不同结果），
  在修复后的代码上它会通过**，且它检查的**必须是本次改动真正改变的那个机制**。
  自检一句话：**「删掉修复本身，它会不会照样通过？」**
- **验证必须标注三选一，且只对前两类声称「已通过机械验证」**：
  `命令`（可自动判真伪）/ `注入`（需临时改代码制造条件）/ `评审`（只能人工看）。
  **不要给评审类条目编造一个「命令」。**
- **凡「修复前后取值相同」的检查，一律标为回归项**，不得当验收项。
- **凡「把 X 移到别处执行」类修复，注入点要跟着 X 走**——插在原地等于没插。
- **「回归风险」里出现「需确认 / 需扫一遍 / 必须核对 / 需逐处确认 / 需前后端同步改」
  这类动作词的，必须二者其一**：**提升为「验证」块里的一条**，或**显式声明
  「接受该风险，不验证」并给出理由**。**不允许只停留在「回归风险」的文字里**——
  它不在任何执行口径内，写在那里等于没写。
- **判据要锚在「会真正变化」的符号上**：锚**定义**（`struct` / `impl`）而非**引用**（`use`）；
  锚稳定词元而非会随行文变动的整句措辞。**引用任何符号前先 `grep` 确认它存在。**
- **写脚本的硬约定**：`grep -c` **零匹配时退出码为 1** ⇒
  一律 `n=$(grep -c … || true); [ "$n" = "期望值" ]`，**不要用 `set -e` 或 `&&` 串**。
- **实测结论必须标注成立条件**：profile / feature / target。
  本仓 release 是 `panic = "abort"` ⇒ 凡「靠 Drop 在展开时复位」的结论**仅 dev / test 可见**。
- **批量改文件走「两遍式」**：先全量断言「每条锚点恰好命中 1 次」，**全过才写盘**；
  锚点取 `git show HEAD:<file>` 的**原文**（格式化工具可能早已折行）。
  ⚠️ 两遍式**拦不住「同一行多个替换对互相覆盖」** ⇒ 内层必须基于**工作副本**，并**逐行 diff 复核**。
- **改完文档立刻复跑 `node tools/doc-table-audit.mjs`**（表格列数 + 自指型断言），
  并做一次**指针体检**：`见 §…` 要能解析到唯一目标。
  **「指针存在」≠「指针指对」≠「内容相符」**——三级递进，实测失效**全卡在后两级**。

## 前端架构备忘

- 结构：popup/settings 双页体系，脚本"分区在前、入口最后"，命名镜像
  （`popup-{devices,audio}.js ↔ settings-{devices,audio}.js`），全部 JS 带标准头注释
  （四要素：文件职责 / 加载序 N/N · 提供：… / 依赖：…）
- **invoke 双轨是有意设计，勿"统一"**：两页**都已是「惰性 + 防御式」**（P2-3 改造后），
  差别在**取用形式与「未就绪」的表达能力**：
  - popup 页走 common.js 挂在 `window` 上的 `getInvoke()`——返回**函数或 `null`**，
    调用方以 `const invoke = getInvoke(); if (!invoke) return;` 判空降级；
  - settings 页（含 settings-about.js）**裸用 `invoke(...)`**，它绑定到 common.js 顶层的
    `const invoke = (...args) => …` 箭头函数（经典脚本的顶层 `const` **不是 `window` 属性**，
    但同页后续脚本可见，属**跨脚本全局词法绑定**）。该包装内部**每次调用重新解析**
    `window.__TAURI__.core`，未就绪时返回 **rejected Promise**，交给调用方既有的
    `.catch()` / `try-catch` 降级。
  ⇒ 关键差异：`getInvoke()` 能表达「未就绪」（`null`，可判空跳过），裸 `invoke` 不能
  （只能给一个被拒的 Promise）。**故不要"统一"成一种写法。**
  ⚠️ 两页的顶层代码都**隐式依赖 common.js 先执行**（settings 页顶层就调
  `registerContextMenu(...)`，popup 页顶层就调 `initTheme()` / `loadDevices()`）。
  **实测**把 common.js 排到最后：`check.mjs` 仍报「前端完整性检查通过」——第 2 类审计的
  声明池**按页汇总且无序**，看不见顺序；而运行时报
  `Uncaught ReferenceError: registerContextMenu is not defined @settings.js:120`。
  词法 `const` 同理（顶层调 `invoke` 得 `ReferenceError: invoke is not defined`——
  定义脚本执行前**该全局绑定根本不存在**，是 `not defined` 而**不是** TDZ）。
  **重排加载序或迁移文件时须保持各自语义。**
- 已否决路线：方案乙 ESM 迁移（触发重启条件：前端规模翻倍 / 多人协作 /
  config 共享实际出 bug；届时可先考虑 config 抽为经典脚本单例的廉价中间路线）
- 材质系统收敛（删除 settings-general 回调手动三件套）暂缓，
  下次因其他原因动材质代码时顺手做并实测闪烁

## 项目 Wiki 维护

开发者知识库位于 GitHub Wiki（独立仓库，页面即 `PeriTray.wiki.git`），
主仓内不存副本。完整规范见 WIKI「Wiki-维护规范」页，此处仅列强制义务：

- **不随 commit / push 同步**；以下九类变更落地时必须同步更新对应 Wiki 页
  （模块增删→03、命令/事件增删→05、架构决策→02、重大踩坑定案→06/07 按模板、
  否决路线状态变化→09、工具链变化→08、新前端页面→04、
  **新增 / 修订架构纪律→13**、**验收方法论有新的教训→14**），AGENTS.md 与 Wiki
  双处内容以 AGENTS.md 为权威源回改
- 发版走「发布流程」第 5 步校准轮（正式版必做）
- wiki 工作区固定在主仓并列目录 `../PeriTray.wiki`，勿用系统临时目录；
  推送前自检清单见 WIKI 规范 §5

## 开发与调试

- **本地运行**：仓库根目录 `npm run tauri dev`（首次需编译）；改 Rust 源会被
  dev 监听自动重建重启，改 dist 前端同样热生效
- **调试开关**：环境变量 `PM_DEV_OPEN_SETTINGS=1` 启动时延迟 1.5s 自动打开
  设置窗口（main.rs），用于自动化验证设置页脚本加载与初始化
- **日志**：写入 `<可写根>/logs/debug_YYYYMMDD.log`（保留策略设为「每次仅保留一次」时
  为 `debug_once_<pid>.log`）；分「标准/详细」两级，级别关闭时 `append_log` 不落盘；
  设置页「通用 → 日志」可开关/调级；排查启动问题先看 `[main] startup complete`
- **持久化路径一律经 `process::writable_root()`，不得直接用 `exe_dir()`**：日志、
  `config.toml`、`data/` 三者都挂在可写根下。**NSIS**（Tauri 默认 per-user，装在
  `%LOCALAPPDATA%`）= exe 同目录，**一字不变**；**MSIX**（Store 包）= 容器 `LocalState`
  `%LOCALAPPDATA%\Packages\<包家族名>\LocalState` —— 包安装目录
  `C:\Program Files\WindowsApps\<包家族名>` **只读、且不在重定向表内**，往里写是**静默失败**
  （配置写不进 → 只记标准级日志 → 日志也写不进 → 只剩 stderr，GUI 全无感）。
  ⚠️ **别改用 `%LOCALAPPDATA%\<标识>`**：包身份下它被虚拟化重定向，**虚拟路径本身不存在**，
  而 `explorer.exe` 不是打包进程、按字面路径找 ⇒ 「查看日志」按钮照样失效。
  **评审检查项**：新增任何落盘路径时，先问「MSIX 下这个目录可写吗？」
- **远程校验**：push 到 main 与 PR 由 CI 工作流（.github/workflows/ci.yml）
  复跑本地闸门全套（check.mjs / rustfmt / cargo check -D warnings / cargo test），
  Rust 工具链按仓库根的 `rust-toolchain.toml` 安装并断言生效版本（见「提交自动闸门」）

# tools/ —— 闸门、诊断与验收脚本

> 单一来源：闸门清单与纪律见 `AGENTS.md`「提交自动闸门（强制）」，机制与踩坑见 Wiki
> [08-工程化与工具链](https://github.com/oneday5799/PeriTray/wiki/08-%E5%B7%A5%E7%A8%8B%E5%8C%96%E4%B8%8E%E5%B7%A5%E5%85%B7%E9%93%BE)。
> 本文件只回答一个问题：**每个文件是什么、什么时候该用它。**

## 一、闸门类（**承重，删不得**）

| 文件 | 干什么 | 谁调用它 |
|---|---|---|
| `check.mjs` | 前端完整性**七类校验**（HTML 引用双向一致 / 跨文件调用审计 / 同名全局函数遮蔽 / `node --check` / BOM / 五处版本号一致 / Toast 契约） | pre-commit、CI、`verify-final.sh` |
| `check-clippy.mjs` | `cargo clippy` 闸门：**`-D warnings` + 存量基线 `-A` + 3 条显式开启**（`await_holding_lock` 等）。条数以本文件的 `BASELINE_ALLOW` 为准 | pre-commit（带 `--optional`）、CI（不带，硬失败） |
| `check-rust-warnings.mjs` | 「零**源码**警告」判据：区分 rustc 的 lint 警告与 **Cargo 自己的产物 I/O 诊断**（本机增量目录锁被拒 ⇒ os error 5 的假红） | pre-commit（**唯一调用点**）；CI 走 `RUSTFLAGS: -D warnings` |
| `doc-table-audit.mjs` | 文档体检：表格列数 + 自指型断言（`§N.x` 活指针 / 裸占位符） | CI、pre-commit 手动、`verify-final.sh` |
| `doc-table-audit.selftest.md` | **故意损坏**的夹具：断言体检判据仍承重（须「退出码 1 且恰好 4 个问题」） | `verify-final.sh` 第 2 段 |
| `pre-commit` | 提交钩子本体。⚠️ **不随仓库走**，重克隆后须 `cp tools/pre-commit .git/hooks/pre-commit` | git |

## 二、诊断类

| 文件 | 干什么 |
|---|---|
| `cdp-eval.mjs` | 通过 **CDP**（WebView2 的调试端口）向运行中的应用求值。⚠️ 日志不可用时的兜底手段；端口持有者是 `msedgewebview2.exe` ⇒ **身份用端口不用 PID** |

## 三、整改验收类（一次性，⚠️ 见下方「为什么没删」）

`verify-final.sh` 是入口（`sh tools/verify-final.sh`），它串起四段：
闸门段 → 文档段 → 脚本段 → MANUAL 段（评审类条目，脚本不做）。

| 文件 | 覆盖 |
|---|---|
| `verify-batch-0.sh` / `-1.sh` / `-2.sh` | 2026-09 审查整改的前三批判据 |
| `verify-batch-4.sh` | 第四批（58 条）。⭐ **2026-09-28 实跑抓到真违规**：v1.4.0-beta.1 新增的任务栏代码里有 12 处裸 `.lock()`，违反「除 `state.rs` 外无例外」 |
| `local/verify-b14.mjs` / `local/verify-l1l2.mjs` | 两条已结案缺陷（B14 快照吞改动 / L1·L2 裸访问 `__TAURI__`）的一次性验收器。**已被移入 `local/`**：不被任何脚本调用，且其观测通道（无头 Edge `--dump-dom`）在本机输出 0 字节 ⇒ 只会 `exit 2` |

### 为什么没删掉这批脚本

三条理由，都是**实跑过**才敢下的结论（2026-09-28）：

1. **它们还能跑，而且有用**：`batch-0/1/2/4` 实跑分别 1 / 3 / 7 / 58 条 PASS，
   `batch-4` 当场抓到上面那条 `.lock()` 真违规——**所有常设闸门都没抓到它**
   （`check.mjs` 不看 Rust 源码，clippy 的 `mutex_atomic` 也不管裸 `.lock()`）。
   「看着过时」和「没价值」是两件事，判据是**跑一遍看它说什么**。
2. **它们是那批整改的账**：判据写死的是「修复前转红 / 修复后转绿」的具体机制，
   删了就等于删掉「当初凭什么说修好了」的证据（Wiki 12 只记结论，不记判据）。
3. **`§x` 指针指向已删除的《修复方案》**：`verify-final.sh` 头部已写明这一点并给出
   现行出处（Wiki 12 / 13 / 14），**保留原指针正是为了还能对账**。

⚠️ **唯一真该修的时点是判据本身腐烂时**（比如绑定到已改名的符号）。发现某条判据
恒真或恒假，就地修或删，**不要**因为「脚本碍眼」而删——`batch-4` 已经证明了它的价值。

## 四、本机脚本类

- `local/` —— 45 个自包含脚本（`probe_*` / `verify-*` / `inject-*` / `shot-*` / `measure-*` / `diag-*`）
  + 上述两条归档的 `verify-*.mjs`。⛔ **全是本机口径**（写死 DPI 125%、任务栏 2560 宽、具体设备名）
  ⇒ 换机器必须重跑并复核期望值。见 `local/README.md`。
- `skills/peri-tray-release.md` —— 发版 playbook。

## 五、命名约定

- `check-*` = **常设闸门**（进 pre-commit / CI）；`verify-*` = 整改批次验收（一次性）。
- ⚠️ 两个家族的**文件名会撞**（`tools/verify-final.sh` 与 `tools/local/verify-*.py`），
  区别只在目录：**仓库根的是闸门链的一环，`local/` 里的是本机一次性探针**。
- `*_test` / `*_selftest` = 判据自身的承重性夹具，**不是**测试用例。

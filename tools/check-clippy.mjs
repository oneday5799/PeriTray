/**
 * Rust 静态检查闸门 —— `cargo clippy` 的**唯一入口**（CI 与 pre-commit 共用）
 *
 * 运行：`node tools/check-clippy.mjs`（硬失败）／`node tools/check-clippy.mjs --optional`
 *       （未装 clippy 时告警跳过，pre-commit 用；CI 一律硬失败）
 *
 * ── 为什么单独成脚本 ────────────────────────────────────────
 * CI 与 `tools/pre-commit` 都要跑**同一条** clippy 命令，而它带 20 条存量基线
 * （见下 `BASELINE_ALLOW`）。本仓反复吃过「同一份清单写在两处、改一处漏一处」的
 * 亏（`AGENTS.md` ↔ Wiki 的 invoke 双轨描述、主方案 §5.3 ↔ §6.3 的 0/1 冲突），
 * 故把命令行收敛到本文件一处，CI 与钩子都只调用它。
 *
 * ── 为什么不直接用 `cargo clippy -- -D warnings` ─────────────────
 * 2026-09-18 在当前 HEAD 上实测：默认 lint 集报 **88 条**（bin 44 个唯一位置，
 * 另加 test 编译单元的重复计数）。裸 `-D warnings` 会让 CI **首次运行即红**；
 * 而这 88 条**全是风格类**（`redundant_closure` 16 / `field_reassign_with_default` 16 /
 * `manual_clamp` 8 / `manual_c_str_literals` 8 / `type_complexity` 5 …），
 * 与本次代码审查的 40 条发现**零交集** ⇒ 在「收尾批」里批量改它们**纯风险无收益**
 * （`manual_clamp` 改 `.clamp()` 还有 `min > max` 时 panic 的语义差异，
 * `derivable_impls` 直指 P1-7 刚整过的 `Config::default`）。
 * 故采用**显式基线**：存量按 lint 粒度封存，新增违规一律拦下；
 * **每修掉一条，就从 `BASELINE_ALLOW` 删一条**（基线即待办清单）。
 *
 * ── 为什么另有三条 `-D` ────────────────────────────────────
 * `BASELINE_ALLOW` 之外**显式开启**三条与本仓头号约束「持锁区只能做纯内存操作」
 * （见 `AGENTS.md`）直接相关的 lint，实测当前均为 **0 命中**，可立刻当防复发断言：
 *   · `clippy::await_holding_lock`        —— 持锁跨 `.await`
 *   · `clippy::await_holding_refcell_ref` —— 持 `RefCell` 借用跨 `.await`
 *   · `clippy::mutex_atomic`              —— 该用原子量却用 `Mutex<bool>`
 *
 * ── 已知边界（**勿当成「Rust 侧已闭合」**，与 `AGENTS.md`「防护边界」互为表里）──
 * · `-A` 基线里的 20 条**对新增代码同样放行**——它们是「封存存量」，
 *   **不是「这类问题不重要」**。要真拦住，得先修完存量再删对应 `-A`。
 * · 实测**默认集不含** `.lock().unwrap()` 的对应 lint：`clippy::unwrap_used`
 *   属 restriction 组、**默认关闭**；显式开启后全仓 **94 条**
 *   （`unwrap_used` 44 + `expect_used` 50）⇒ **不可能 `-D`**。
 *   即：审查报告点名的「`.lock().unwrap()`」这一类，clippy **默认不覆盖**。
 *   （主方案 §8.3 第 5 条与决策 10 称 clippy「已覆盖 `.lock().unwrap()`」——
 *   **该立论经实测不成立**，见计划文件 §7.4 的订正。）
 * · `let _ =` 丢弃 must_use：`SingleFlightGuard` **没有 `#[must_use]`**
 *   （`src-tauri/src/state.rs`），故 `let _ = guard` 抓不到；而 `let _x = guard`
 *   的 `unused_variables` 按语言规范**主动豁免** ⇒ **这一类同样没有机械防线**
 *   （出处：主方案 §8.3 第 3 条 + §二 的对照表；报告 P3-8）。
 *   ⚠️ 注意别引成「避坑清单第 18 条」——那条讲的是「闸门边界要写在闸门旁边」，
 *   与本条的「无机械检查」是两件事（本次初稿引错过一次，已订正）。
 * · 异步上下文里的 `thread::sleep`（全仓 24 处）：clippy **无对应 lint**。
 * · **基线写错 lint 名不会静默失效**：`-D warnings` 会把 `unknown_lints`
 *   升级为 `error[E0602]` ⇒ 但**改基线必须实跑一次**，否则 CI 会以
 *   「unknown lint」的形式变红（已实测）。
 * · **工具链浮动**：CI 用 `dtolnay/rust-toolchain@stable`。Rust 若往
 *   `clippy::all` 里新增 lint，CI 会**无故变红**——这与既有的
 *   `RUSTFLAGS: -D warnings` 属**同一类既有风险**，本次既未新增也未修复。
 * · `significant_drop_in_scrutinee`（nursery，默认关）实测命中 1 处
 *   （`src/windows.rs:146` 的 `if let Some(info) = *lock_unpoisoned(...)`，
 *   守卫活到 `if let` 结束）——该处 body 只有 `return info;`，**当前无害**；
 *   未启用该 lint（nursery 组随版本变动，不宜进闸门），仅此登记。
 */

import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import path from "node:path";

const OPTIONAL = process.argv.includes("--optional");

/**
 * 存量基线（2026-09-18 实测）。数字是**实测**命中数，不是估计；
 * 合计 88 条（含 test 单元的重复计数）。修掉一条就删一行。
 */
const BASELINE_ALLOW = [
  "clippy::redundant_closure", // 16
  "clippy::field_reassign_with_default", // 16
  "clippy::manual_clamp", //  8
  "clippy::manual_c_str_literals", //  8
  "clippy::type_complexity", //  5
  "clippy::clone_on_copy", //  4
  "clippy::too_many_arguments", //  4
  "clippy::unwrap_or_default", //  3
  "clippy::useless_conversion", //  2
  "clippy::needless_borrow", //  2
  "clippy::needless_question_mark", //  2
  "clippy::derivable_impls", //  2
  "clippy::needless_return", //  2
  "clippy::nonminimal_bool", //  2
  "clippy::redundant_locals", //  2
  "clippy::manual_range_contains", //  2
  "clippy::doc_lazy_continuation", //  2
  "clippy::redundant_guards", //  2
  "clippy::missing_transmute_annotations", //  2
  "clippy::manual_is_multiple_of", //  2
];

/** 显式开启（实测当前 0 命中）：与「持锁区只能做纯内存操作」直接相关。 */
const ENFORCE_DENY = [
  "clippy::await_holding_lock",
  "clippy::await_holding_refcell_ref",
  "clippy::mutex_atomic",
];

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");
const CARGO_DIR = path.join(ROOT, "src-tauri");

// 组装 argv（不拼字符串）：`-D warnings` 在前，其后逐条 `-A` 封存存量，
// 最后逐条 `-D` 开启高价值项。clippy 的 lint 级别「后写的赢」，顺序即优先级。
const args = ["clippy", "--all-targets", "--", "-D", "warnings"];
for (const lint of BASELINE_ALLOW) args.push("-A", lint);
for (const lint of ENFORCE_DENY) args.push("-D", lint);

// 先探针：区分「clippy 没装」与「clippy 报了违规」——两者的处置完全不同。
let clippyAvailable = true;
try {
  execFileSync("cargo", ["clippy", "--version"], { cwd: CARGO_DIR, stdio: "pipe" });
} catch {
  clippyAvailable = false;
}

if (!clippyAvailable) {
  if (OPTIONAL) {
    console.warn(
      "[check-clippy] ⚠️ 未找到 cargo clippy 组件，本次**跳过** Rust clippy 闸门。\n" +
        "[check-clippy]    本机补装：rustup component add clippy\n" +
        "[check-clippy]    注意：CI 上这一步是硬失败，跳过只影响本地提交。"
    );
    process.exit(0);
  }
  console.error(
    "[check-clippy] ❌ 未找到 cargo clippy 组件，Rust clippy 闸门无法执行。\n" +
      "[check-clippy]    本机补装：rustup component add clippy"
  );
  process.exit(1);
}

console.log("[check-clippy] 运行 cargo clippy --all-targets（基线 20 条 + 显式开启 3 条）...");
const res = spawnSync("cargo", args, { cwd: CARGO_DIR, stdio: "inherit" });

if (res.error) {
  console.error(`[check-clippy] ❌ 无法启动 cargo：${res.error.message}`);
  process.exit(1);
}
if (res.status !== 0) {
  console.error(
    "\n[check-clippy] ❌ clippy 闸门未通过，提交/CI 被拦截。\n" +
      "[check-clippy]    若命中的是 `BASELINE_ALLOW` 里那 20 类**存量**风格问题，\n" +
      "[check-clippy]    正确做法是**就地修掉并从基线删掉那一行**，不要新加 `-A`；\n" +
      "[check-clippy]    若是 `unknown lint`（E0602），说明基线里的 lint 名写错了。"
  );
  process.exit(res.status ?? 1);
}

console.log("[check-clippy] Rust clippy 检查通过");

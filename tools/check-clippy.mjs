/**
 * Rust 静态检查闸门 —— `cargo clippy` 的**唯一入口**（CI 与 pre-commit 共用）
 *
 * 运行：`node tools/check-clippy.mjs`（硬失败）／`node tools/check-clippy.mjs --optional`
 *       （未装 clippy 时告警跳过，pre-commit 用；CI 一律硬失败）
 *
 * ── 为什么单独成脚本 ────────────────────────────────────────
 * CI 与 `tools/pre-commit` 都要跑**同一条** clippy 命令，而它带一份存量基线
 * （见下 `BASELINE_ALLOW`，**条数随修复递减，不要在任何地方写死**）。
 * 本仓反复吃过「同一份清单写在两处、改一处漏一处」的
 * 亏（`AGENTS.md` ↔ Wiki 的 invoke 双轨描述、主方案 §5.3 ↔ §6.3 的 0/1 冲突），
 * 故把命令行收敛到本文件一处，CI 与钩子都只调用它。
 *
 * ── 为什么不直接用 `cargo clippy -- -D warnings` ─────────────────
 * 2026-09-18 在当前 HEAD 上实测：默认 lint 集报 **88 条**（bin 44 个唯一位置，
 * 另加 test 编译单元的重复计数）。裸 `-D warnings` 会让 CI **首次运行即红**；
 * 而这 88 条**全是风格类**（当时的分布：`redundant_closure` 16 /
 * `field_reassign_with_default` 16 / `manual_clamp` 8 / `manual_c_str_literals` 8 /
 * `type_complexity` 5 …——**当前剩余量只看下面的 `BASELINE_ALLOW`，别引本段数字**），
 * 与本次代码审查的 40 条发现**零交集** ⇒ 在「收尾批」里批量改它们**纯风险无收益**
 * （当时记的两条顾虑：`manual_clamp` 改 `.clamp()` 有 NaN 语义差异；`derivable_impls`
 * 被记成「直指 P1-7 刚整过的 `Config::default`」——**后者是错的**：实测命中的是
 * `LogRetention` 这个枚举的 `Default`，与 `Config::default` 无关，批 3 已订正并改掉）。
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
 * · `-A` 基线里的那些 lint**对新增代码同样放行**——它们是「封存存量」，
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
 * · **工具链版本已固定（2026-09-19，本节此前写「CI 用浮动 @stable」）**：版本由仓库根的
 *   `rust-toolchain.toml` 指定；`ci.yml` / `release.yml` 都**从该文件读 channel 再安装**，
 *   并在同一步断言「生效工具链 == 文件里的值」⇒ 本地与 CI 同版，
 *   **「Rust 往 clippy::all 加新 lint ⇒ CI 无故变红」这一类风险已消除**。
 *   代价是换成了「改 channel 的人必须自己把本脚本跑一遍」——故 `rust-toolchain.toml`
 *   文件头写了升级三步（跑五道闸门 / 看 CI 十步全绿 / 同步 Wiki 08）。
 *   立此条的实测依据：浮动的 `@stable` 推进到 1.98 后新增 `chunks_exact_to_as_chunks`，
 *   命中**存量**代码 `src-tauri/src/app_icon.rs:477`，在 `-D warnings` 下成为编译错误
 *   （已修，`d8a483c`）。
 * · `significant_drop_in_scrutinee`（nursery，默认关）实测命中 1 处
 *   （`src/windows.rs:146` 的 `if let Some(info) = *lock_unpoisoned(...)`，
 *   守卫活到 `if let` 结束）——该处 body 只有 `return info;`，**当前无害**；
 *   未启用该 lint（nursery 组随版本变动，不宜进闸门），仅此登记。
 */

import { execFileSync, spawnSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import fs from "node:fs";
import path from "node:path";

const OPTIONAL = process.argv.includes("--optional");

/**
 * 存量基线（2026-09-18 首次实测；此后按批修复，**修掉一条就删一行**）。
 *
 * 注释里的数字是 2026-09-20 实测的**唯一位置数**——同一处源码会在 bin 与 test 两个
 * 编译单元里各报一次，故 `cargo` 输出的原始告警行数约为它的两倍。两种口径都写清，
 * 免得下一个人对着 `cargo clippy` 的条数说「对不上」。
 *
 * ⚠️ 每次改动本数组都必须**实跑一次** `node tools/check-clippy.mjs`：lint 名写错会以
 *    `unknown lint (E0602)` 的形式让 CI 变红，而不是静默失效（已实测）。
 */
const BASELINE_ALLOW = [
  // ⚠️ 这是**有意保留的决定，不是遗漏**（2026-09-20 批 3 逐处判过）：
  // 两处命中是 `dedup.rs::try_insert`（14 参 / 107 行 / 2 个调用点）与
  // `wmi_query.rs::query_pnp_devices`（8 参 / 115 行 / 1 个调用点）。
  // 清掉它得把参数收进结构体——那是**真重构**（约 222 行 + 3 个调用点），
  // 不是等价改写，且正落在设备去重 / WMI 枚举这段语义最讲究的代码上。
  // 收尾批里做它纯风险无收益；要真做，应作为独立批次、配独立的回归判据。
  "clippy::too_many_arguments", //  2
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

// 工具链版本自检（**只告警，不拦截**）。版本固定在仓库根的 rust-toolchain.toml，
// CI 那一步会硬断言；本地此前没有任何判据 —— 若被 RUSTUP_TOOLCHAIN 之类覆盖，
// 本次 clippy 结果就不代表 CI 的结果（这正是 2026-09-19「本地绿、CI 红」的成因）。
const pinned = (() => {
  try {
    const text = fs.readFileSync(path.join(ROOT, "rust-toolchain.toml"), "utf8");
    const m = text.match(/^\s*channel\s*=\s*"([^"]*)"/m);
    return m ? m[1] : null;
  } catch {
    return null; // 文件不存在：说明是旧检出，不做判断
  }
})();

if (pinned) {
  try {
    const active = execFileSync("rustup", ["show", "active-toolchain"], {
      cwd: CARGO_DIR,
      encoding: "utf8",
    }).trim();
    if (!active.startsWith(pinned)) {
      console.warn(
        `[check-clippy] ⚠️ 生效工具链「${active}」与 rust-toolchain.toml 固定的「${pinned}」不一致\n` +
          "[check-clippy]    本次 clippy 结果**不代表 CI 的结果**。常见原因：设了 RUSTUP_TOOLCHAIN。"
      );
    }
  } catch {
    // rustup 不在 PATH（非 rustup 管理的工具链）：跳过自检，不影响闸门本身
  }
}

console.log(
  `[check-clippy] 运行 cargo clippy --all-targets（基线 ${BASELINE_ALLOW.length} 条` +
    ` + 显式开启 ${ENFORCE_DENY.length} 条）...`
);
const res = spawnSync("cargo", args, { cwd: CARGO_DIR, stdio: "inherit" });

if (res.error) {
  console.error(`[check-clippy] ❌ 无法启动 cargo：${res.error.message}`);
  process.exit(1);
}
if (res.status !== 0) {
  console.error(
    "\n[check-clippy] ❌ clippy 闸门未通过，提交/CI 被拦截。\n" +
      `[check-clippy]    若命中的是 BASELINE_ALLOW 里那 ${BASELINE_ALLOW.length} 类` +
        "**存量**风格问题，\n" +
      "[check-clippy]    正确做法是**就地修掉并从基线删掉那一行**，不要新加 `-A`；\n" +
      "[check-clippy]    若是 `unknown lint`（E0602），说明基线里的 lint 名写错了。"
  );
  process.exit(res.status ?? 1);
}

console.log("[check-clippy] Rust clippy 检查通过");

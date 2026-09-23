/**
 * Rust 警告闸门 —— 「零源码警告」判据的**唯一入口**（CI 与 pre-commit 共用）
 *
 * 运行：`node tools/check-rust-warnings.mjs`
 *
 * ── 为什么需要本脚本（判据的真实缺陷）────────────────────────────
 * 原先 `tools/pre-commit` 与 CI 都用同一句朴素判据：`cargo check | grep "^warning"`。
 * 该判据**无法区分「rustc 的 lint 警告」与「Cargo 自己的产物 I/O 诊断」**，
 * 于 2026-09-24 在提交 T3-2 时**误拦**（下称「锁文件噪音」）：
 *
 *   warning: error deleting lock file for incremental compilation session
 *            directory `...\incremental\PeriTray-<hash>\s-<hash>.lock`: 拒绝访问。 (os error 5)
 *   warning: `PeriTray` (bin "PeriTray") generated 1 warning
 *
 * 实测结论（**均已复现，非推测**）：
 *   · 该次 `cargo check` 的**源码警告为 0**；`CARGO_INCREMENTAL=0 cargo check`
 *     下 `grep "^warning"` **完全无输出**（exit 1）⇒ 拦的是环境噪音，不是源码问题。
 *   · 第二行 `generated 1 warning` 是**汇总行**，统计的正是第一行那条噪音
 *     ⇒ 即使只排除首行，朴素判据仍会命中汇总行。
 *   · **挪走/删除物理锁文件无效**：连续三次运行报的**是同一个固定文件名**，
 *     而该文件当时**不在磁盘上**（已移出并验证）⇒ 该名字来自 Cargo 的
 *     会话清理记录，与文件是否存在**无关**。故「清掉残留锁文件」这条路**走不通**
 *     （本机另受 `[safe-delete]` 守卫限制，`Remove-Item` 一律被拦）。
 *
 * ── 判据设计（**为什么不是简单地按 level=="warning" 过滤**）──────────
 * 关键实测：**锁文件噪音在 JSON 流里同样是一条 `compiler-message`**
 * （`"$message_type":"diagnostic"`, `level:"warning"`）——
 * 它是 Cargo 借 diagnostic 通道发出的，**不是普通 stdout 打印**。
 * ⇒ 仅按 `level=="warning"` 判定**仍会误拦**（这是本次差点写错的第二个坑）。
 *
 * 两条警告的 JSON 形态对照（实测取样）：
 *
 *   维度              | 锁文件噪音                  | 真 lint 警告
 *   ------------------|-----------------------------|-------------------------------
 *   message.code      | null                        | {"code":"unused_variables",...}
 *   message.spans     | []   （空）                 | 非空，且带 file_name
 *   message.message   | "error deleting lock file…" | lint 描述文本
 *
 * 故判据取**两个条件同时成立**（保守，宁可不报也不误报）：
 *   ① `message.code !== null`     —— 必须是一条**有 lint 名**的诊断；
 *   ② 某个 span 带 `file_name`    —— 必须**指向一个源文件**。
 * 噪音因为 code=null 且 spans=[] 被两条同时挡掉。
 *
 * ── 与 CI 的关系 ────────────────────────────────────────────
 * CI 的 `cargo check` 另带 `RUSTFLAGS: -D warnings`（由 rustc 自判，天然免疫噪音）。
 * 本脚本**不**引入 `RUSTFLAGS`，因为实测在本机全量重编依赖时，
 * `-D warnings` 会炸在第三方 `schemars`（`E0107`）⇒ **不可移植**。
 * 本脚本用 `--message-format=json` 达到同等语义（只认 rustc 的 lint 诊断）
 * 且**不需要全量重编**。
 *
 * ⚠️ 本脚本的判据**必须与 CI 能达成的结论一致**：CI 上若出现真警告，
 * `-D warnings` 会直接编译失败（exit != 0）；本脚本则报出警告列表并非零退出。
 * 两侧都拦得住，只是通道不同。
 */

import { spawnSync } from "node:child_process";

const CARGO_DIR = "src-tauri";

/**
 * 从一行 JSON 里判断它是否为「指向源文件的 rustc lint 警告」。
 *
 * 详见文件头「判据设计」：**两个条件必须同时成立**，
 * 否则 Cargo 的锁文件噪音（`code: null` + `spans: []`）会被误判为警告。
 */
function isSourceLintWarning(entry) {
  if (entry?.reason !== "compiler-message") return false;
  const msg = entry.message;
  if (!msg || msg.level !== "warning") return false;
  // ① 必须有 lint code（噪音的 code 是 null）
  if (!msg.code || !msg.code.code) return false;
  // ② 必须有指向源文件的 span（噪音的 spans 是空数组）
  if (!Array.isArray(msg.spans) || msg.spans.length === 0) return false;
  return msg.spans.some((s) => typeof s?.file_name === "string" && s.file_name.length > 0);
}

function main() {
  const res = spawnSync("cargo", ["check", "--message-format=json"], {
    cwd: CARGO_DIR,
    encoding: "utf8",
    maxBuffer: 64 * 1024 * 1024,
    // ⚠️ Windows 上 cargo 是 .cmd，必须 shell:true 才能被找到。
    // ⛔⛔ **stdin 必须 `ignore`**：本机（2026-09 实测，与 `tools/check.mjs:291`
    //    同一根因）子进程 stdin 走 pipe 时 node 恒抛 `EBUSY`（`spawnSync cmd.exe EBUSY`）
    //    ⇒ 判据会对**所有**输入假红。切勿删这个 `ignore` 改回默认 pipe。
    stdio: ["ignore", "pipe", "pipe"],
    shell: true,
  });

  if (res.error) {
    console.error(`[rust-warnings] 无法运行 cargo check：${res.error.message}`);
    process.exit(1);
  }

  // 编译失败（含 CI 的 -D warnings 升级成的 error）一律硬失败。
  if (res.status !== 0) {
    console.error("[rust-warnings] cargo check 失败，提交被拦截");
    if (res.stderr) process.stderr.write(res.stderr);
    process.exit(1);
  }

  const warnings = [];
  for (const line of (res.stdout || "").split("\n")) {
    const trimmed = line.trim();
    if (!trimmed.startsWith("{")) continue;
    let entry;
    try {
      entry = JSON.parse(trimmed);
    } catch {
      continue; // 非 JSON 行（理论不会出现）忽略
    }
    if (isSourceLintWarning(entry)) warnings.push(entry.message);
  }

  if (warnings.length > 0) {
    for (const w of warnings) {
      process.stderr.write(w.rendered || `warning: ${w.message}\n`);
    }
    console.error(
      `[rust-warnings] cargo check 存在 ${warnings.length} 条源码警告（要求零警告），提交被拦截`
    );
    process.exit(1);
  }

  console.log("[rust-warnings] cargo check 无源码警告");
}

main();

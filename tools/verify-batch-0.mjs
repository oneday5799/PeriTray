// 第零批验收脚本（P3-14：RAII 守卫命名约定）
//
// 用法：在仓库根执行 `node tools/verify-batch-0.mjs`
// 依据：主方案 §6.3「每批的验收清单写法」的实操建议——「把每批的验收段写成
//       tools/verify-batch-N.mjs，随该批提交一起入库，这样将来任何人回看都能直接重跑」。
// 迁移注记：由 verify-batch-0.sh 迁为 .mjs（工具脚本语言收敛）；判据集合与 PASS/FAIL
//   输出口径不变，迁移对账 diff 为空。语言分工 → Wiki 08 §1.1。
//
// ⚠️ 两条硬约定（主方案 §6.3 末尾，写脚本时踩过的坑）：
//   1) `grep -c` 在**零匹配**时退出码为 **1** ⇒ 一律「赋值给变量再比较」。
//      迁为 .mjs 后该坑不再适用（自实现计数不抛异常），但口径保持：命中 0 = 合法值。
//   2) 「回归项」≠「验收项」：回归项防的是**今后新增误用**，它在修复前后取值相同，
//      **不能**作为「这批修好了」的证据。本脚本把两类分开标注。
//
// ⚠️ 本批是**纯文档修复**（P3-14 只改 AGENTS.md 的命名约定，不动代码）⇒
//    验收项只能是**评审**（主方案 §6.3 末尾的边界声明：凡改动本身是文档/规范的条目，
//    「不要给它们编造一个『命令』」）。脚本给出定位命令 + 需人工确认的原文。

import { readFileSync, readdirSync, statSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = join(dirname(fileURLToPath(import.meta.url)), "..");
process.chdir(ROOT);

let fail = 0;
const chkEq = (name, got, want) => {
  if (String(got) === String(want)) console.log(`PASS  ${name}（实得 ${got}）`);
  else {
    console.log(`FAIL  ${name}（期望 ${want}，实得 ${got}）`);
    fail = 1;
  }
};
const chkGe = (name, got, floor) => {
  const g = Number(got);
  if (Number.isFinite(g) && g >= floor) console.log(`PASS  ${name}（实得 ${got} ≥ ${floor}）`);
  else {
    console.log(`FAIL  ${name}（期望 ≥ ${floor}，实得 ${got}）`);
    fail = 1;
  }
};

// grep -c 的口径：**行计数**（一行命中多次只计 1）。pattern 传字符串 = 字面子串、
// 传 RegExp = 正则（用于原 BRE 里 `.` 语义必须保留的判据，如「忽略 .Result.」）。
const readLines = (p) => readFileSync(join(ROOT, p), "utf-8").split("\n");
const grepCount = (pat, file) =>
  readLines(file).filter((l) => (pat instanceof RegExp ? pat.test(l) : l.includes(pat))).length;

// grep -rn --include="*.rs" 的口径：递归收 src-tauri/src 下全部 .rs（与 git 跟踪面一致）。
// ⚠️ 输出路径必须用 `/` 分隔（grep -rn 在 Git Bash 下如此），否则与 sh 时代的对账单不一致
const walkRs = (dir) => {
  const out = [];
  for (const e of readdirSync(dir, { withFileTypes: true })) {
    const p = `${dir}/${e.name}`;
    if (e.isDirectory()) out.push(...walkRs(p));
    else if (e.name.endsWith(".rs")) out.push(p);
  }
  return out;
};

console.log("== 第零批（P3-14）验收 ==");

// ── 回归项（不是验收项）────────────────────────────────────────────
// 主方案 §6.3 原写「应为 7」，那是**当时的**基线（bluetooth.rs 6 处 + tray.rs:285）。
// 2026-09-18 实测为 **25 处 / 7 个文件**——后续批次（P3-13 把 SingleFlightGuard 上移
// state.rs、P2-11 风格统一等）新增了合法的内层 Drop-only 守卫，基线自然增长。
//
// ⚠️ **本条是「计数式」检查，没有判别力**：它数的是文本出现次数，分不出
// 「外层守卫误加下划线」（真问题）与「内层 `let _guard = guard;`」（正确写法）。
// 若把它当硬闸门，**任何新增的合法守卫都会让它转红**，结果是训练人忽略本脚本。
// 故这里只打印计数与构成供人工比对，**不失败**。
const reLetUnderscore = /let _[a-zA-Z_][a-zA-Z0-9_]* = /;
const hits = [];
for (const f of walkRs("src-tauri/src")) {
  readLines(f).forEach((l, i) => {
    if (reLetUnderscore.test(l)) hits.push([f, i + 1]);
  });
}
console.log(
  `INFO  回归项：\`let _<name> = \` 计数 = ${hits.length}（2026-09-18 基线 25；主方案 §6.3 写的 7 已过时）`
);
console.log("      按文件构成：");
const byFile = new Map();
for (const [f] of hits) byFile.set(f, (byFile.get(f) || 0) + 1);
// sh 管道 `sort | uniq -c` 的口径：uniq -c 把计数**右对齐到 7 列**再接空格与路径
for (const [f, n] of [...byFile.entries()].sort((a, b) => (a[0] < b[0] ? -1 : 1)))
  console.log(`        ${String(n).padStart(7)} ${f}`);
console.log("      ⇒ 若计数变化，人工确认新增的是「内层 Drop-only 守卫」还是「外层误加下划线」。");

// ── 验收项（评审类）─────────────────────────────────────────────────
// ⚠️ AGENTS.md 现只留判据索引、全文下沉 Wiki 13 §3，旧短语「闭包捕获的守卫」已不在主仓
//    ⇒ 判据改锚索引行的**稳定词元**（「外层绑定不得以」），防「改措辞即假红」。
chkGe(
  "验收项：AGENTS.md 已含 RAII 守卫命名约定索引（外层绑定不得以 _ 开头；全文在 Wiki 13 §3）",
  grepCount("外层绑定不得以", "AGENTS.md"),
  1
);

console.log("  ---- 需人工评审的原文（AGENTS.md 架构纪律索引行；全文在 Wiki 13 §3）----");
// `grep -n "RAII 守卫" AGENTS.md | sed 's/^/  /'` 的口径：命中行带 `行号:` 前缀，整体再缩进 2 空格
const ag = readLines("AGENTS.md");
ag.forEach((l, i) => {
  if (l.includes("RAII 守卫")) console.log(`  ${i + 1}:${l}`);
});
console.log("  ⇒ 人工确认三件事（对照 Wiki 13 §3）：① 约定说的是「外层守卫」而非「所有守卫」；");
console.log("     ② 明确写了内层 Drop-only 守卫（如 let _guard = guard;）是正确写法；");
console.log("     ③ 指向了报告 P3-14。");

console.log("");
if (fail === 0) {
  console.log("第零批验收：通过（回归项为 INFO，需人工比对）");
} else {
  console.log("第零批验收：**未通过**");
  process.exit(1);
}

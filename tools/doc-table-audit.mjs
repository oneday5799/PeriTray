#!/usr/bin/env node
/**
 * 文档体检工具 —— 把《修复方案》§8.3 的两条收尾动作做成可重复执行的机械判据：
 *
 *   第 15 条「列数体检」：**同一张 Markdown 表内列数必须一致**，且表内行内代码里
 *     不得出现**裸竖线**（GFM 下表格单元格解析先于行内代码，裸 `|` 会被当成分隔符
 *     导致渲染错位）。第 26 轮那次损坏（避坑清单某行粘连上一行尾部、`|` 数 6）就是
 *     这类问题——**文字全对、任何内容检查都发现不了**。
 *
 *   第 18 条「自指型断言体检」：交付稿里不得残留 `§x.x` 这类**活指针**（占位符从未
 *     实例化）；`见 §…` 指针要能解析到存在的章节。
 *
 * 用法：
 *   node tools/doc-table-audit.mjs              # 默认扫 docs/code-review/*.md
 *   node tools/doc-table-audit.mjs <文件...>    # 指定文件
 *   node tools/doc-table-audit.mjs tools/doc-table-audit.selftest.md
 *                                               # 判据自测夹具：**期望退出码 1、问题恰好 4 个**
 *                                               # （少于 4 即某条判据已失效；由 verify-final.sh 调用）
 *
 * 退出码：0 = 无硬失败；1 = 有硬失败（列数不一致 / 表内裸竖线 / 活指针）。
 *
 * ⚠️ 判据设计的四个坑（都是实测踩出来的）：
 *   ① **列数判据必须是「同一张表内一致」**，不能写成「所有编号行都是 N 列」——
 *      后者会把文档里本就存在的 5/6/7 列表全判成异常（首次跑产出 79 行假阳性）。
 *   ② 必须**排除行内代码里的转义竖线** `\|`，否则 `f(move \|\| …)` 会被算成列分隔符。
 *   ③ **正则必须覆盖任意 `§N.x`**，不能只写 `§2.x` / `§x.x`——后者会漏掉 `§6.x`。
 *      这正是主方案 §9.4 手工体检漏报的原因：它自称「0 处活指针」，却因窄模式没数到 `§6.x`。
 *   ④ **必须区分「使用」与「提及」**：`§N.x` 落在 `「…」` 内是**引用旧文本**（元文本），
 *      不构成指针——本仓的复核记录大量引用「详见 §2.x」来说明那个**已修好**的断指针。
 *      只有**行内代码之外、且不在引号内**的裸 `§N.x` 才算活指针。首次跑出 16 处，
 *      逐条看过确认 15 处属此类元文本，**仅 1 处（`§6.x`）是真裸引用**
 *      （已就地实例化为 `§6.1~§6.4`）⇒ 这条判据若不收窄，假阳性率就是 15/16。
 *      ⚠️ 但**引号内不计为硬失败 ≠ 引号内不必看**：脚本会把它们汇总成一行 INFO，
 *         以防有人把真断指针塞进引号里绕过闸门。
 */

import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";

const DOC_DIR = "docs/code-review";

/** 去掉转义竖线后数「列数」（`| a | b |` → 2） */
const cols = (line) => line.replace(/\\\|/g, "").split("|").length - 2;

/** 把行内代码的内容替换为等长空格，用于「不在行内代码里」的判定 */
const maskInlineCode = (line) => line.replace(/`[^`]*`/g, (m) => " ".repeat(m.length));

/**
 * 把中文引号 `「…」` / `『…』` 的**内容**替换为等长空格（保留引号本身）。
 * 用途：区分「提及」与「使用」——引号内的 `§N.x` 是在**引用旧文本**，不是指针。
 */
const maskQuoted = (line) =>
  line
    .replace(/「[^」]*」/g, (m) => "「" + " ".repeat(m.length - 2) + "」")
    .replace(/『[^』]*』/g, (m) => "『" + " ".repeat(m.length - 2) + "』");

/** 活指针形态：任意 `§N.x` / `§x.x`（⚠️ 不能只写 `§2.x`，见文件头 ③） */
const PLACEHOLDER_RE = /§[0-9A-Za-z]+\.x/g;

/** 找出表内行内代码里的**裸竖线**（未转义的 `|`） */
function barePipesInCode(line) {
  const found = [];
  const re = /`[^`]*`/g;
  let m;
  while ((m = re.exec(line)) !== null) {
    const span = m[0];
    const all = (span.match(/\|/g) || []).length;
    const esc = (span.match(/\\\|/g) || []).length;
    if (all - esc > 0) found.push(span);
  }
  return found;
}

const isTableRow = (line) => {
  const t = line.trim();
  return t.length > 1 && t.startsWith("|") && t.endsWith("|");
};

function auditFile(path) {
  const lines = readFileSync(path, "utf8").split(/\r?\n/);
  const problems = [];
  const infos = [];

  // ── 1) 列数体检：按「连续表格行」分块，块内列数必须一致 ──
  let block = null; // { first, cols }
  let tableBlocks = 0;
  for (let i = 0; i < lines.length; i++) {
    const line = lines[i];
    if (isTableRow(line)) {
      const n = cols(line);
      if (block === null) {
        block = { first: i + 1, cols: n };
        tableBlocks++;
      } else if (n !== block.cols) {
        problems.push(
          `列数不一致：表起始第 ${block.first} 行是 ${block.cols} 列，第 ${i + 1} 行却是 ${n} 列`,
        );
      }
    } else {
      block = null;
    }
  }

  // ── 2) 表内行内代码里的裸竖线（只查表格行——正文/代码块里的裸竖线无害）──
  for (let i = 0; i < lines.length; i++) {
    if (!isTableRow(lines[i])) continue;
    for (const span of barePipesInCode(lines[i])) {
      problems.push(`表内裸竖线：第 ${i + 1} 行 \`${span.slice(0, 60)}\`（需写成 \\| ）`);
    }
  }

  // ── 3) 自指型断言：活指针（行内代码之外、且**不在引号内**的 `§N.x` 占位符）──
  // 引号内的属「提及」（引用旧文本），不计硬失败，但汇总成一行 INFO 保留可见性，
  // 以防有人把真断指针塞进引号里绕过闸门。
  const quotedHits = [];
  for (let i = 0; i < lines.length; i++) {
    const code = maskInlineCode(lines[i]);
    const quoted = maskQuoted(code);
    PLACEHOLDER_RE.lastIndex = 0;
    let m;
    while ((m = PLACEHOLDER_RE.exec(code)) !== null) {
      const at = m.index;
      const fullyQuoted = quoted.slice(at, at + m[0].length).trim() === "";
      if (fullyQuoted) {
        quotedHits.push(`${i + 1}`);
      } else {
        problems.push(
          `活指针：第 ${i + 1} 行残留占位符 ${m[0]}` +
            `（行内代码之外、且不在引号内的裸占位符属未实例化指针）`,
        );
      }
    }
  }

  // ── 4) INFO：`见 §…` 指针清单 + 目标章节存在性（内容相符需人工）──
  const headings = lines
    .filter((l) => /^#{1,6}\s/.test(l))
    .map((l) => l.replace(/^#{1,6}\s*/, "").trim());
  const pointers = new Map();
  for (let i = 0; i < lines.length; i++) {
    const masked = maskInlineCode(lines[i]);
    for (const m of masked.match(/见 §[0-9A-Za-z.]+/g) || []) {
      if (!pointers.has(m)) pointers.set(m, i + 1);
    }
  }
  for (const [ptr, line] of pointers) {
    const num = ptr.replace(/^见 §/, "");
    // 章节标题形态：`### 7.6 …` / `## 六、…` / `### 8.3 …`
    const resolved = headings.some((h) => h.startsWith(num + " ") || h.startsWith(num + "、") || h.startsWith(num));
    if (!resolved) infos.push(`指针 ${ptr}（第 ${line} 行）未能解析到章节标题，请人工确认`);
  }

  return { path, tableBlocks, problems, infos, quotedHits, pointerCount: pointers.size };
}

const args = process.argv.slice(2);
const files =
  args.length > 0
    ? args
    : readdirSync(DOC_DIR)
        .filter((f) => f.endsWith(".md"))
        .map((f) => join(DOC_DIR, f))
        .sort();

let totalProblems = 0;
let totalBlocks = 0;
console.log(`[doc-table-audit] 体检 ${files.length} 份文档`);
for (const f of files) {
  const r = auditFile(f);
  totalBlocks += r.tableBlocks;
  totalProblems += r.problems.length;
  console.log(
    `  ${r.problems.length === 0 ? "PASS" : "FAIL"}  ${r.path}：表格块 ${r.tableBlocks}，` +
      `问题 ${r.problems.length}，\`见 §…\` 指针 ${r.pointerCount}`,
  );
  for (const p of r.problems) console.log(`        ✗ ${p}`);
  for (const i of r.infos) console.log(`        · INFO ${i}`);
  if (r.quotedHits.length > 0) {
    console.log(
      `        · INFO 引号内的 §N.x 字样 ${r.quotedHits.length} 处` +
        `（第 ${r.quotedHits.join(",")} 行）：按「提及 ≠ 使用」判为元文本，**不计硬失败**；` +
        `若其中混入真断指针，需人工确认`,
    );
  }
}
console.log(`[doc-table-audit] 合计：表格块 ${totalBlocks}，硬失败 ${totalProblems}`);
process.exit(totalProblems === 0 ? 0 : 1);

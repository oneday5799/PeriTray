/**
 * 注释清理的**机械部分**：剥离四类不该写进代码的内容。
 * 完整判据见 AGENTS.md「代码与注释风格 → 注释体例」与 Wiki 13 §19。
 *
 * ⚠️ 本脚本**只做可机械判定的剥离**，不做「重写句子」——
 *   凡是「删掉会让句子不成话」的，一律留给人工逐块处理。
 *   判据：每条规则都能被一条规则表达式完全描述；人读一遍 diff 即可确认。
 *
 * 用法：
 *   node tools/strip-comment-narrative.mjs --dry <文件...>   # 只预览
 *   node tools/strip-comment-narrative.mjs --write <文件...>  # 落盘
 */
import fs from "node:fs";

const DRY = process.argv.includes("--dry");
const WRITE = process.argv.includes("--write");
const files = process.argv.slice(2).filter((a) => !a.startsWith("--"));

// ── 规则表 ──
// ⛔⛔ **只允许删「词元」，不允许删「带可选括号的从句」**。
//   初版每条规则都写成 `（?[^（）()]{0,8}…[^（）()]{0,40}?）?` 想连括号一起吃掉，
//   结果正则贪婪扩张：单是「外部项目出处」一条就把 `TokenBar 的缺陷正是**不校验却直接标成功**`
//   啃成 `的缺陷正是**不校验却直接标成功**）`，而报告却说「改动 2300 行」。
//   ⇒ **改动的行数远多于命中数，就是规则在乱吃的信号**。
//   ⇒ 现在每条规则只匹配一个**确定的短词元**，删完由人去读 diff。
const RULES = [
  {
    name: "日期",
    re: /\s*20\d\d-\d\d-\d\d/g,
    why: "日期不改变判据，且必然漂移",
  },
  {
    name: "外部项目名",
    re: /\s*(?:FluentFlyout|StockBar|TokenBar|OpenRazer)(?=[\s的（(])/g,
    why: "仓库需要的是「怎么查」的指针，不是别人的项目名",
  },
  {
    name: "报障叙事",
    re: /(?:用户实测报|用户报|用户反馈|用户实测发现|报的就是)[「『][^」』]*[」』]/g,
    why: "一次性事件，不是判据；复现步骤归验收脚本",
  },
  {
    name: "用户要求转述",
    re: /(?<=（)用户\s*(?=[：:])/g,
    why: "括号里只剩「用户要求：…」时，去掉主语后仍是可读的判据",
  },
];

/** 只处理注释行；代码行一个字节都不碰。 */
function isComment(line) {
  return /^\s*(\/\/|\/\*|\*)/.test(line);
}

function strip(text) {
  const hits = [];
  let out = text;
  for (const r of RULES) {
    out = out.replace(r.re, (m) => {
      hits.push(r.name);
      return "";
    });
  }
  // ── 定点清理：只动**中文标点相邻处**的碎屑 ──
  // ⛔⛔ **不得做全局空白压缩**（如 `[ \t]{2,}(?=\S)` → " "）：
  //   注释正文里大量使用**对齐缩进**（`//!   1. **建窗**`），
  //   压缩会把它们全部拉平 ⇒ 实测「改动 2265 行」而真实命中只有 27 处。
  out = out
    .replace(/（\s+/g, "（")                    // 「（ 修：」→「（修：」
    .replace(/\s+）/g, "）")                    // 「… ）」→「…）」
    .replace(/「\s+/g, "「")
    .replace(/（\s*[：:]\s*/g, "（")            // 「（：「…」）」→「（「…」）」
    .replace(/（\s*）/g, "")                    // 空括号
    .replace(/用户\s+(?=[：:]?\s*(?:要求|规定|明确|指定|选定|选|定稿|确认|实测报|报))/, "")
    .replace(/用户\s*(?=[：:])/, "")
    .replace(/实测报出/g, "报出")
    .replace(/[ \t]+$/, "");
  return { out, hits };
}

let totalHits = 0;
for (const f of files) {
  const src = fs.readFileSync(f, "utf-8");
  const lines = src.split("\n");
  const perRule = {};
  const changed = [];
  for (let i = 0; i < lines.length; i++) {
    if (!isComment(lines[i])) continue;
    const { out, hits } = strip(lines[i]);
    if (out !== lines[i]) {
      for (const h of hits) perRule[h] = (perRule[h] || 0) + 1;
      totalHits += hits.length;
      changed.push([i + 1, lines[i], out]);
    }
  }
  const name = f.replace(/\\/g, "/");
  console.log(`\n═══ ${name} ═══ 改动 ${changed.length} 行`);
  for (const [k, v] of Object.entries(perRule)) {
    const rule = RULES.find((r) => r.name === k);
    console.log(`   ${k}: ${v} 处 —— ${rule.why}`);
  }
  if (DRY) {
    for (const [ln, a, b] of changed.slice(0, Number(process.env.PREVIEW || 12))) {
      console.log(`\n   L${ln}\n   − ${a.trim()}\n   + ${b.trim()}`);
    }
    if (changed.length > Number(process.env.PREVIEW || 12)) {
      console.log(`\n   …… 另有 ${changed.length - Number(process.env.PREVIEW || 12)} 行（PREVIEW=999 看全部）`);
    }
  }
  if (WRITE) {
    for (const [ln, , b] of changed) lines[ln - 1] = b;
    fs.writeFileSync(f, lines.join("\n"));
    console.log(`   → 已写入 ${f}`);
  }
}
console.log(`\n合计剥离 ${totalHits} 处`);

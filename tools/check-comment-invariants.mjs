/**
 * 承重注释不变量的回归闸门。
 *
 * ── 为什么需要它 ────────────────────────────────────────────────
 * 本仓**没有任何闸门检查注释内容**（`check.mjs` 只查前端完整性，
 * `doc-table-audit` 只查 `AGENTS.md` / `README.md`）。
 * 而「清理冗余注释」这件事本身有**不可逆的风险**：删掉的注释里
 * 混着两种东西 ——
 *   · 承重的判据（「⛔ 持锁区不得调 `SetWindowText`」这类，删了就没人知道为什么）
 *   · 纯叙事（「报『…』」「早先写成…改成…」这类，删了只丢历史）
 * 两者在文本上**无法机械区分** ⇒ 必须先把「承重的那些」锚定成可校验的清单，
 *   否则「清理」就退化成「凭感觉删」，而删错的代价是**静默的知识丢失**。
 *
 * ── 它保证什么、不保证什么 ──────────────────────────────────────
 * ✅ 保证：登记表里的每个 (文件, 锚点) 仍在、该处 `⛔`/`⭐` 数量没减少、
 *         全仓总数不低于 FLOOR、`§` 指针能解析到真实章节。
 * ⛔ **不保证**「留下的解释都是对的」—— 哪些解释值得留是判断题，只能靠评审。
 *    登记表文件头已写明这条边界，避免它被当成「注释已达标」的证明。
 *
 * 用法：`node tools/check-comment-invariants.mjs`
 */
import fs from "node:fs";
import path from "node:path";

const ROOT = path.resolve(import.meta.dirname, "..");
const REGISTRY = path.join(ROOT, "tools", "comment-invariants.txt");
const SCAN_DIRS = ["src-tauri/src", "tools"];
const MARKERS = ["⛔", "⭐"];
const WIKI_DIR = path.resolve(ROOT, "..", "PeriTray.wiki");

const MOD_RE = /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([A-Za-z0-9_]+)/;
const FN_RE = /^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:unsafe\s+)?(?:const\s+|static\s+)?fn\s+([A-Za-z0-9_]+)/;
const CONST_RE = /^\s*(?:pub\s+)?(?:const|static)\s+([A-Z][A-Z0-9_]*)\s*:/;
const IMPL_RE = /^\s*impl\s/;
const COMMENT_RE = /^\s*(\/\/|\/\*|\*)/;

function eachFile(dir, out = []) {
  for (const e of fs.readdirSync(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) {
      if (e.name === "target" || e.name === "node_modules") continue;
      eachFile(p, out);
    } else if (e.name.endsWith(".rs") || e.name.endsWith(".mjs")) {
      out.push(p);
    }
  }
  return out;
}

/** 扫一个文件，返回 Map<锚点, Map<标记, 次数>> 与该文件的注释行数组。 */
function scanFile(abs) {
  const rel = path.relative(ROOT, abs).replace(/\\/g, "/");
  const lines = fs.readFileSync(abs, "utf-8").split("\n");
  const marks = new Map();
  const commentLines = [];
  let anchor = "<file>";
  for (let i = 0; i < lines.length; i++) {
    const l = lines[i];
    let m = MOD_RE.exec(l);
    if (m) { anchor = `mod ${m[1]}`; continue; }
    m = FN_RE.exec(l);
    if (m) { anchor = `fn ${m[1]}`; continue; }
    m = CONST_RE.exec(l);
    if (m) { anchor = `const ${m[1]}`; continue; }
    if (IMPL_RE.test(l)) anchor = `${anchor.split(" fn ")[0]} impl`;
    if (!COMMENT_RE.test(l)) continue;
    commentLines.push(l);
    const hit = MARKERS.find((k) => l.includes(k));
    if (!hit) continue;
    const byMark = marks.get(anchor) || new Map();
    byMark.set(hit, (byMark.get(hit) || 0) + 1);
    marks.set(anchor, byMark);
  }
  return { rel, marks, commentLines };
}

// ── 扫全仓 ──
const scanned = new Map();
let total = 0;
for (const dir of SCAN_DIRS) {
  const abs = path.join(ROOT, dir);
  if (!fs.existsSync(abs)) continue;
  for (const f of eachFile(abs)) {
    const r = scanFile(f);
    scanned.set(r.rel, r);
    for (const byMark of r.marks.values()) {
      for (const n of byMark.values()) total += n;
    }
  }
}


// ── --write：用当前扫描结果重写登记表（**唯一来源**，避免两套扫描逻辑漂移）──
if (process.argv.includes("--write")) {
  const rows = [];
  for (const rec of scanned.values()) {
    for (const [anchor, byMark] of rec.marks) {
      for (const [mark, n] of byMark) rows.push([rec.rel, anchor, mark, n]);
    }
  }
  rows.sort((a, b) => (a[0] + a[1]).localeCompare(b[0] + b[1]) || a[2].localeCompare(b[2]));
  const head = fs
    .readFileSync(REGISTRY, "utf-8")
    .split("\n")
    .filter((l) => l.startsWith("#"))
    .join("\n");
  const body = rows.map(([f, a2, mk, n]) => `${f}\t${a2}\t${mk}\t${n}`).join("\n");
  fs.writeFileSync(REGISTRY, `${head}\n\n${body}\n\n# FLOOR\t⛔+⭐ 总数下界\t${total}\n`);
  console.log(`[check-comment-invariants] 已重写登记表：${rows.length} 个锚条目，FLOOR=${total}`);
  process.exit(0);
}

// ── 读登记表 ──
if (!fs.existsSync(REGISTRY)) {
  console.error("⛔ 缺少 tools/comment-invariants.txt（承重不变量登记表）");
  process.exit(1);
}
const regText = fs.readFileSync(REGISTRY, "utf-8");
const entries = [];
let floor = null;
for (const raw of regText.split("\n")) {
  if (!raw) continue;
  // ⛔ FLOOR 行以 `#` 开头（它是注释性元数据），**必须在跳过注释之前解析** ——
  //   早先先 `startsWith("#")` 就 continue ⇒ 永远读不到 ⇒ 恒报「没有 FLOOR 行」。
  const cols = raw.replace(/^#\s*/, "").split("\t");
  if (cols[0] === "FLOOR") {
    floor = Number(cols[cols.length - 1]);
    continue;
  }
  if (raw.startsWith("#")) continue;
  const parts = cols;
  if (parts.length < 4) continue;
  entries.push({ file: parts[0], anchor: parts[1], mark: parts[2], n: Number(parts[3]) });
}
if (floor === null || !Number.isFinite(floor)) {
  console.error("⛔ 登记表里没有 FLOOR 行");
  process.exit(1);
}

// ── 断言 ①②③ ──
const problems = [];
for (const e of entries) {
  const rec = scanned.get(e.file);
  if (!rec) {
    problems.push(`${e.file}：登记表引用的文件已不存在`);
    continue;
  }
  const byMark = rec.marks.get(e.anchor);
  if (!byMark) {
    problems.push(`${e.file}：锚点 \`${e.anchor}\` 已不存在（符号被删或改名）`);
    continue;
  }
  const have = byMark.get(e.mark) || 0;
  if (have < e.n) {
    problems.push(
      `${e.file} @ ${e.anchor}：${e.mark} 由 ${e.n} 降到 ${have} ⇒ 该处的判据被删了`,
    );
  }
}
if (total < floor) {
  problems.push(`全仓 ${MARKERS.join("+")} 总数 ${total} < 下界 ${floor} ⇒ 有承重判据被删`);
}

// ── 断言 ④：`§` 指针可解析 ──
// ⚠️ Wiki 目录不在主仓内（`../PeriTray.wiki`），缺失时**降级为 INFO**而不是硬失败
//    ——「拿不到输入」不等于「有问题」，也不等于「没问题」。
const pointers = new Map();     // `${page}|${sec}`
let barePointers = 0;
// ⛔⛔ 早先的正则是「有 Wiki 前缀就用它、没有就**默认当 Wiki 13**」⇒
//   `Spec §0.4`（device_identity.rs / commands.rs 里指向**外部 Spec 文档**）
//   被误判成 Wiki 指针 ⇒ 恒红。
//   ⇒ 改为**只硬校验显式带 `Wiki NN` 前缀的**；裸 `§x.y` 无法判定指向哪份文档，
//     报 INFO 而不是失败 —— 「拿不到输入」不等于「有问题」。
const PTR_RE = /Wiki\s*(\d{2})[^\n]{0,16}?§(\d+(?:\.\d+)*)/g;
const BARE_RE = /§(\d+(?:\.\d+)*)/g;
for (const rec of scanned.values()) {
  for (const line of rec.commentLines) {
    PTR_RE.lastIndex = 0;
    let m;
    while ((m = PTR_RE.exec(line)) !== null) pointers.set(`${m[1]}|${m[2]}`, 1);
    BARE_RE.lastIndex = 0;
    const masked = line.replace(PTR_RE, " ");
    while (BARE_RE.exec(masked) !== null) barePointers++;
  }
}
const info = [];
if (fs.existsSync(WIKI_DIR)) {
  const headings = new Map();   // `${page}|${secTop}` -> 真实标题串
  for (const f of fs.readdirSync(WIKI_DIR)) {
    const mm = /^(\d{2})-.+\.md$/.exec(f);
    if (!mm) continue;
    const page = mm[1];
    const txt = fs.readFileSync(path.join(WIKI_DIR, f), "utf-8");
    for (const h of txt.split("\n")) {
      const hm = /^#{2,4}\s+(\d+(?:\.\d+)*)/.exec(h);
      if (hm) headings.set(`${page}|${hm[1]}`, h.trim());
    }
  }
  for (const key of pointers.keys()) {
    const [page, sec] = key.split("|");
    const top = sec.split(".")[0];
    if (!headings.has(`${page}|${sec}`) && !headings.has(`${page}|${top}`)) {
      problems.push(
        `代码注释里的指针 \`§${sec}\`（默认指 Wiki ${page}）在 Wiki 里查不到对应章节`,
      );
    }
  }
  info.push(
    `已校验 ${pointers.size} 个带 Wiki 前缀的 § 指针（Wiki 目录存在）；` +
      `另有 ${barePointers} 处裸 §x.y 指向别处（Spec 等），未校验`,
  );
} else {
  info.push(
    `⛔ 指针校验**已跳过**：找不到 ${WIKI_DIR} —— 属「拿不到输入」，既不算通过也不算失败`,
  );
}

// ── 报告 ──
console.log(
  `[check-comment-invariants] 登记 ${entries.length} 个锚条目；` +
    `全仓 ${MARKERS.join("+")} = ${total}（下界 ${floor}）`,
);
for (const s of info) console.log(`[check-comment-invariants] ${s}`);
if (problems.length) {
  console.error(`[check-comment-invariants] ⛔ ${problems.length} 项不通过：`);
  for (const p of problems.slice(0, 20)) console.error(`  · ${p}`);
  if (problems.length > 20) console.error(`  …… 另有 ${problems.length - 20} 项`);
  process.exit(1);
}
console.log("[check-comment-invariants] 承重不变量登记表校验通过");

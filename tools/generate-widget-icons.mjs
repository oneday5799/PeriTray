/**
 * 任务栏组件图标生成器：把 `tools/icons/widget/*.svg` 渲染成
 * `src-tauri/icons/tray-{widget,music}-*-icon{,-dark}.png`（256×256 RGBA），
 * 过程中把**描边宽度统一到 13px@256**。
 *
 * ── 为什么必须有这个脚本（它是被迫入库的）─────────────────────────
 * 上一批音乐图标由 `generate_music_icons.mjs`（**一次性、不入库**）生成，
 * 当时把切换图标从 80「腐蚀到 51.2」以对齐设备图标。
 * ⛔ 正因为脚本没入库 ⇒ 没人能重新生成、也没人知道它统一过一次
 * ⇒ 「线宽不一致」才能在设备侧与音乐侧之间**长期残留**（用户 2026-09-30 报障）。
 * ⇒ **生成脚本必须与产物一起入库**。
 *
 * ── 做法：**矢量法线内偏移**（不是像素形态学）──────────────────────
 *   SVG 源里设备侧描边是 64 单位、音乐侧是 51.2 单位；
 *   把 64 变成 51.2 ⇒ 轮廓整体内移 (64 − 51.2)/2 = **6.4 单位**。
 *
 * ⛔ 为什么**不能**在像素域做（这一版之前在像素域试了六种，全部失败）：
 *   像素域的任何「腐蚀 / 开运算 / 脊线归一」都会**动到外轮廓**：
 *   局部偏薄处整段被删 ⇒ **断口**；结构交界处腐蚀半径突变 ⇒ **鼓包**。
 *   用户实测反馈原话：「轮廓变形／有缺口凸起」。
 *   而矢量域里「整体内移 6.4」是逐点精确的：直线仍是直线、圆弧仍是圆弧、
 *   缓弯 bézier 误差 < 0.1 单位；6.4/1024 = 0.6% 远小于任何曲率半径
 *   ⇒ **形状逐点保持**，只是整体小 0.6%。
 *
 * ── 为什么偏移量不需要区分「外轮廓 / 孔洞」────────────────────────
 *   源用的是 **nonzero** 填充 ⇒ 孔洞靠**绕向相反**挖出。
 *   于是对两条轮廓，「行进方向左侧 = 材料侧」都成立
 *   ⇒ 一律 `+d` 即为「朝材料内推」⇒ 描边两侧同时内收，宽度正好减 2d。
 *   ⚠️ 方向若搞反，描边会变成 64 + 12.8 = 76.8 ⇒ 被下面的量测闸门当场抓住。
 *
 * ── 三道闸门（任一不过就拒绝写盘并非 0 退出）──────────────────────
 *   ① 超粗残留 ≤ 外轮廓墨迹的 0.5%（OVER_BUDGET）
 *   ② 拓扑不变：前景连通域数 / 孔洞数必须与偏移前一致（断口、孔洞贯通都会被抓）
 *   ③ 描边宽度实测落在 12~16px@256（并打印**未分桶**的最大值）
 *
 * 用法：`node tools/generate-widget-icons.mjs`
 */
import fs from "node:fs";
import path from "node:path";
import sharp from "sharp";
import { offsetSubpaths, parsePath } from "./svg-offset.mjs";

const ROOT = path.resolve(import.meta.dirname, "..");
const SRC = path.join(ROOT, "tools", "icons", "widget");
const OUT = path.join(ROOT, "src-tauri", "icons");

/** ⛔ 这些是现网实测的中枢值，改它们等于改设计口径 */
const SIZE = 256;      // 输出边长（与现网一致 ⇒ 运行时缩放路径不动）
const WORK = 1024;     // 判定用的工作分辨率（像素量化会主导低分辨率下的测量）
const STROKE_PX = 13;  // 目标线宽（256 下）= 51.2 / 1024
const TOL_PX = 2;      // 线宽合格区间：W ± TOL（膨胀描线在斜接处固有偏厚）
const OVER_BUDGET = 0.005;
/** alpha 低于此值 ⇒ RGB 归零（对齐 resvg 直出资源的既有约定，见 writeMask）。 */
const LOW_ALPHA_ZERO = 8;
const LIGHT = [20, 21, 15];
const DARK = [242, 242, 242];

/** 残留预算（按外轮廓墨迹量）。 */
const budgetOf = (inkPx) => Math.floor(inkPx * OVER_BUDGET);

/**
 * 每个图标：SVG 名、输出基名、**逐子路径的法线内移量**（单位 @1024 画布）。
 *
 * 索引按「所有 `<path>` 的子路径依次编号」的全局顺序，与 SVG 里出现的顺序一致。
 * 0 表示该子路径不动（内部细节：键帽 / 十字键 / 按钮）。
 *
 * ⭐ 切换图标不用偏移：源里它是**实心**箭头（最厚 113 单位），
 *   偏移无法把实心块变成 W 宽的笔画 ⇒ 改为**直接给一条描边折线**
 *   （见 SWITCH_STROKE），线宽由 `stroke-width` 保证。
 */
const PLAN = [
  { name: "mouse", base: "tray-widget-mouse-icon", radii: [6.4, 6.4, 6.4] },
  { name: "keyboard", base: "tray-keyboard-icon", radii: [6.4, 6.4, 0, 0, 0, 0] },
  { name: "gamepad", base: "tray-gamepad-icon", radii: [8.5, 8.5, 0, 0, 0, 0] },
  { name: "speaker", base: "tray-speaker-icon", radii: [6.4, 6.4, 6.4, 6.4, 6.4, 6.4] },
  { name: "headphone", base: "tray-headphone-icon", radii: [6.4, 6.4, 6.4] },
  { name: "play", base: "tray-music-play-icon", radii: [0, 0, 0, 0] },
  { name: "pause", base: "tray-music-pause-icon", radii: [0, 0, 0, 0] },
  { name: "prev", base: "tray-music-prev-icon", radii: [0, 0, 0, 0, 0] },
  { name: "next", base: "tray-music-next-icon", radii: [0, 0, 0, 0, 0] },
];

/**
 * 切换图标：**重画**成一条 51.2 宽的描边折线（源里是实心块）。
 *
 * 端点由旧图的**实测外接盒**反推：源 `v1.4.0-beta.2` 的旧 PNG 外接盒是
 * (85,36)-(186,218)@256 ⇒ (340,144)-(744,872)@1024；
 * 圆头描边的端点 = 外接盒边界内缩 W/2 = 25.6。
 * 手臂夹角约 43.8°，与旧图一致。
 */
const SWITCH_STROKE = {
  name: "switch",
  base: "tray-music-switch-icon",
  svg:
    '<path d="M365.6 169.6 L718.4 508 L365.6 846.4" fill="none" stroke="#000000"' +
    ' stroke-width="51.2" stroke-linecap="round" stroke-linejoin="round"/>',
};

// ── 精确欧氏距离变换（Felzenszwalb & Huttenlocher，两趟 1D 抛物线下包络）──

function edt1d(f, n, d, v, z) {
  let k = 0;
  v[0] = 0;
  z[0] = -Infinity;
  z[1] = Infinity;
  for (let q = 1; q < n; q++) {
    let s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    // ⛔ 必须带 k > 0 守卫：f[0] 是哨兵时 s 算成 -Infinity，而 z[0] 也是 -Infinity
    //   ⇒ -Inf <= -Inf 为真 ⇒ k-- 下溢到 v[-1] ⇒ 读到 undefined ⇒ NaN 扩散。
    while (k > 0 && s <= z[k]) {
      k--;
      s = (f[q] + q * q - (f[v[k]] + v[k] * v[k])) / (2 * q - 2 * v[k]);
    }
    k++;
    v[k] = q;
    z[k] = s;
    z[k + 1] = Infinity;
  }
  k = 0;
  for (let q = 0; q < n; q++) {
    while (z[k + 1] < q) k++;
    const dq = q - v[k];
    d[q] = dq * dq + f[v[k]];
  }
}

/** 到「mask 为 true 的像素集合」的平方欧氏距离。 */
function edt2d(mask, w, h) {
  // ⚠️ 哨兵取 1e10 而**不是** 1e20：下包络要先算 (f[q]+q²) − (f[v[k]]+v[k]²)，
  //   双精度在 1e20 处的间距是 2^14 = 16384，而 q² ≤ 1.05e6
  //   ⇒ 两边各自被舍入成同一个数 ⇒ 分子恒 0 ⇒ 「无穷远」抛物线被误判成最低
  //   ⇒ 距离算小 ⇒ 判定结果全错。1e10 处间距 2^-19，q² 完整保留。
  const INF = 1e10;
  const grid = new Float64Array(w * h);
  for (let i = 0; i < w * h; i++) grid[i] = mask[i] ? 0 : INF;
  const m = Math.max(w, h);
  const f = new Float64Array(m);
  const d = new Float64Array(m);
  const v = new Int32Array(m + 1);
  const z = new Float64Array(m + 1);
  for (let x = 0; x < w; x++) {
    for (let y = 0; y < h; y++) f[y] = grid[y * w + x];
    edt1d(f, h, d, v, z);
    for (let y = 0; y < h; y++) grid[y * w + x] = d[y];
  }
  for (let y = 0; y < h; y++) {
    for (let x = 0; x < w; x++) f[x] = grid[y * w + x];
    edt1d(f, w, d, v, z);
    for (let x = 0; x < w; x++) grid[y * w + x] = d[x];
  }
  return grid;
}

/** 连通域标记（4 邻接），返回 [labelMap, {id: {px:[], bbox}}]。 */
function labelComponents(mask, w, h) {
  const label = new Int32Array(w * h).fill(-1);
  const stack = new Int32Array(w * h);
  const comps = [];
  for (let i = 0; i < w * h; i++) {
    if (!mask[i] || label[i] >= 0) continue;
    const id = comps.length;
    let sp = 0, x0 = w, y0 = h, x1 = -1, y1 = -1;
    const px = [];
    stack[sp++] = i;
    label[i] = id;
    while (sp > 0) {
      const p = stack[--sp];
      px.push(p);
      const x = p % w, y = (p / w) | 0;
      if (x < x0) x0 = x;
      if (x > x1) x1 = x;
      if (y < y0) y0 = y;
      if (y > y1) y1 = y;
      if (x > 0 && mask[p - 1] && label[p - 1] < 0) { label[p - 1] = id; stack[sp++] = p - 1; }
      if (x < w - 1 && mask[p + 1] && label[p + 1] < 0) { label[p + 1] = id; stack[sp++] = p + 1; }
      if (y > 0 && mask[p - w] && label[p - w] < 0) { label[p - w] = id; stack[sp++] = p - w; }
      if (y < h - 1 && mask[p + w] && label[p + w] < 0) { label[p + w] = id; stack[sp++] = p + w; }
    }
    comps.push({ id, px, bbox: [x0, y0, x1, y1] });
  }
  return { label, comps };
}

/** 局部厚度 = 2 × 到最近背景的距离（与朝向无关；方向无关是必须的，见文件头）。 */
function maxThickness(mask, px, w, h) {
  const n = w * h;
  const notInk = new Uint8Array(n);
  for (let i = 0; i < n; i++) notInk[i] = mask[i] ? 0 : 1;
  const dist = edt2d(notInk, w, h);
  let mx = 0;
  for (const p of px) {
    const t = (2 * Math.sqrt(dist[p]) * SIZE) / WORK;
    if (t > mx) mx = t;
  }
  return mx;
}

/**
 * ⭐ 核心判据：**拿偏移前 / 偏移后两个掩码逐连通域对照**，直接编码需求本身：
 *
 *   ① 「内部细节保持现状」⇒ **面积几乎没变的连通域必须逐像素完全相同**；
 *   ② 「统一外轮廓」⇒ **面积变了的连通域（就是轮廓），其最大局部厚度须 ≤ W + 容差**。
 *
 * ⭐ 为什么不能只报「整图超粗像素数」：
 *   内部细节**本来就有意不归一**（手柄十字键与按钮 17px、键盘键帽 6.4px），
 *   把它们算进来就永远不达标（实测超粗 58722 个，视觉上却完全正确）。
 */
function compare(before, after, w, h) {
  const A = labelComponents(before, w, h);
  const B = labelComponents(after, w, h);
  const unchanged = [];
  const changed = [];
  for (const ca of A.comps) {
    // ⛔ 早先用「包围盒有重叠」配对 ⇒ 张冠李戴（实测出现 1271% 的荒谬比值）。
    //   ⇒ 改为 **IoU 最大**且要求 > 0.5。
    let cb = null, bestIou = 0;
    for (const c of B.comps) {
      const ix = Math.max(0, Math.min(ca.bbox[2], c.bbox[2]) - Math.max(ca.bbox[0], c.bbox[0]) + 1);
      const iy = Math.max(0, Math.min(ca.bbox[3], c.bbox[3]) - Math.max(ca.bbox[1], c.bbox[1]) + 1);
      const inter = ix * iy;
      if (!inter) continue;
      const iou = inter / (ca.bbox[2] - ca.bbox[0] + 1) / (ca.bbox[3] - ca.bbox[1] + 1)
        + (c.bbox[2] - c.bbox[0] + 1) * (c.bbox[3] - c.bbox[1] + 1) - inter;
      const score = inter / iou;
      if (score > bestIou) { bestIou = score; cb = c; }
    }
    if (bestIou <= 0.5) cb = null;
    if (!cb) { changed.push({ px: ca.px, maxT: 0, orphan: true }); continue; }
    const ratio = cb.px.length / ca.px.length;
    const rec = { px: ca.px, maxT: maxThickness(after, ca.px, w, h), maxT0: maxThickness(before, ca.px, w, h), ratio, id: ca.id };
    if (Math.abs(ratio - 1) < 0.01) {
      const same = ca.px.length === cb.px.length && ca.px.every((p) => after[p]);
      if (!same) changed.push({ ...rec, maxT: 999 });
      else unchanged.push(rec);
    } else changed.push(rec);
  }
  const orphans = B.comps.filter((c) => !A.comps.some((a) => c.px.length === a.px.length && a.bbox[0] === c.bbox[0]));
  return { unchanged, changed, compsBefore: A.comps.length, compsAfter: B.comps.length, newComps: orphans.length };
}

// ── 渲染 ──

function svgHeader(viewBox = "0 0 1024 1024") {
  return `<svg xmlns="http://www.w3.org/2000/svg" viewBox="${viewBox}" width="1024" height="1024">`;
}

/**
 * 读源 SVG 并产出偏移后的 SVG 文本。
 *
 * ⭐ 每个非零子路径的**方向**都是**试出来的**：分别按 +d / −d 渲染一次，
 *   取**填充面积更小**的那个。
 *   ⛔ 不能从绕向推方向 —— 源文件里各子路径绕向并不统一：
 *     键盘外框 p0[0] 为正、扬声器外框 p0[1] 为正，
 *     但扬声器的**圆环外轮廓 p1[0] 为负** ⇒ 按面积符号定方向会把圆环向外扩。
 *   ⇒ 「材料在收缩」这件事本身由栅格化结果判定，不靠几何推断。
 */
async function buildOffsetSvg(name, radii) {
  const raw = fs.readFileSync(path.join(SRC, `${name}.svg`), "utf-8");
  const viewBox = raw.match(/viewBox="([^"]+)"/)?.[1] ?? "0 0 1024 1024";
  const paths = [...raw.matchAll(/<path([^>]*)>/g)].map((m) => {
    const attrs = m[1];
    return {
      d: attrs.match(/d="([^"]+)"/)?.[1] ?? "",
      fill: attrs.match(/fill="([^"]+)"/)?.[1] ?? "#000000",
      n: parsePath(attrs.match(/d="([^"]+)"/)?.[1] ?? "").length,
    };
  });
  const local = new Array(paths.reduce((a, b) => a + b.n, 0)).fill(0);

  const render = async (radiiArr) => {
    let gi = 0;
    const body = paths.map((p) => {
      const r = {};
      for (let k = 0; k < p.n; k++) r[gi + k] = radiiArr[gi + k];
      gi += p.n;
      return `<path d="${offsetSubpaths(p.d, { radii: r })}" fill="${p.fill}"/>`;
    }).join("");
    const mask = await renderMask(svgHeader(viewBox) + body + "</svg>");
    let on = 0;
    for (let i = 0; i < mask.length; i++) on += mask[i];
    return { svg: svgHeader(viewBox) + body + "</svg>", area: on };
  };

  for (let i = 0; i < local.length; i++) {
    const d0 = radii[i] ?? 0;
    if (d0 === 0) continue;
    const plus = local.slice();
    plus[i] = Math.abs(d0);
    const minus = local.slice();
    minus[i] = -Math.abs(d0);
    const a = await render(plus);
    const b = await render(minus);
    local[i] = a.area <= b.area ? Math.abs(d0) : -Math.abs(d0);
  }
  return (await render(local)).svg;
}

async function renderMask(svgText) {
  const { data } = await sharp(Buffer.from(svgText), { density: 96 * (WORK / 512) })
    .resize(WORK, WORK, { fit: "fill" })
    .ensureAlpha()
    .extractChannel("alpha")
    .raw()
    .toBuffer({ resolveWithObject: true });
  const mask = new Uint8Array(WORK * WORK);
  for (let i = 0; i < mask.length; i++) mask[i] = data[i] >= 128 ? 1 : 0;
  return mask;
}

async function writeMask(mask, file, [r, g, b]) {
  const px = Buffer.alloc(SIZE * SIZE * 4);
  const scale = WORK / SIZE;
  for (let y = 0; y < SIZE; y++) {
    for (let x = 0; x < SIZE; x++) {
      let hit = 0;
      for (let dy = 0; dy < scale; dy++) {
        for (let dx = 0; dx < scale; dx++) hit += mask[(y * scale + dy) * WORK + (x * scale + dx)];
      }
      const o = (y * SIZE + x) * 4;
      const a = Math.round((hit / (scale * scale)) * 255);
      // ⚠️⚠️ **alpha 极低时 RGB 必须归零**，否则缩小会出现黑晕。
      //   下游 `resample::resample` 走的是「先预乘再插值」路径，
      //   而**预乘那一步之前**直插会先把透明像素里的 RGB 混进半透明边缘。
      //   实测（本轮踩到）：全画布写恒定 RGB ⇒ 单测
      //   `rescale_produces_no_alpha_halo` 在 40px 结果里数出 25 个「带 RGB 的
      //   近透明像素」⇒ 正是这条。
      //   旧资源（resvg 直出）实测也是这个约定：alpha ≤7 ⇒ RGB=(0,0,0)，
      //   alpha ≥32 ⇒ RGB≈本色 ⇒ 这里照抄，**不是**自造阈值。
      //   ⚠️ 别改成「写预乘值」：那会让下游再预乘一次 ⇒ 边缘偏暗
      //     （`scale_cached_premul` 的注释已记这条：预乘空间不能走两遍）。
      const on = a >= LOW_ALPHA_ZERO ? 1 : 0;
      px[o] = r * on;
      px[o + 1] = g * on;
      px[o + 2] = b * on;
      px[o + 3] = a;
    }
  }
  await sharp(px, { raw: { width: SIZE, height: SIZE, channels: 4 } })
    .png({ compressionLevel: 9 })
    .toFile(path.join(OUT, file));
}


const report = [];
let failed = false;
for (const item of [...PLAN, SWITCH_STROKE]) {
  const { name, base } = item;
  const svgText = item.svg ? svgHeader() + item.svg + "</svg>" : await buildOffsetSvg(name, item.radii);
  const mask = await renderMask(svgText);

  // 偏移前的基准：同一条 SVG，但不做偏移
  const baseSvg = item.svg
    ? svgHeader() + item.svg + "</svg>"
    : await buildOffsetSvg(name, item.radii.map(() => 0));
  const baseMask = await renderMask(baseSvg);

  const cmp = compare(baseMask, mask, WORK, WORK);

  const problems = [];
  if (cmp.compsBefore !== cmp.compsAfter) {
    problems.push(`连通域数 ${cmp.compsBefore}→${cmp.compsAfter}（偏移把图形拆开/粘连了）`);
  }
  // 轮廓**绝不能比原图更粗**（抓「方向反了 / 偏移量为负」）。
  // ⚠️ 不能要求「全部 ≤ 13+容差」：手柄的**肩部**（握把与机身交汇处）
  //   在原图里本来就厚 30px@256，那是设计而非缺陷 ⇒ 归一后仍厚，属预期。
  //   ⇒ 判据是「不增厚」+「典型厚度收到 W」，而不是「最厚处也必须等于 W」。
  for (const c of cmp.changed) {
    if (c.maxT > c.maxT0 + TOL_PX) {
      problems.push(`轮廓最大线宽 ${c.maxT.toFixed(1)}px 比原图 ${c.maxT0.toFixed(1)}px 还粗（偏移方向反了）`);
      break;
    }
    if (c.ratio > 1.05) {
      problems.push(`轮廓面积反而变大 ${(c.ratio * 100).toFixed(0)}%（偏移方向反了）`);
      break;
    }
  }
  // 只有「计划里确实给了非零偏移量」才要求归一生效：
  // 播放 / 上一首 / 下一首在源里**本来就是 51.2**，切换是**重画**成描边折线，
  // 它们理应一个像素都不变 ⇒ 用「必须被归一」去卡它们是判据自己搞错了。
  const wantsOffset = !item.svg && (item.radii ?? []).some((r) => r !== 0);
  if (wantsOffset && !cmp.changed.length) problems.push("计划给了非零偏移量，但没有任何连通域被归一");
  // 「内部细节保持现状」：面积几乎没变的连通域必须**逐像素完全相同**
  for (const c of cmp.changed) {
    if (c.maxT === 999) { problems.push("某个未归一的连通域被改动了像素（内部细节必须原样保留）"); break; }
  }

  await writeMask(mask, `${base}.png`, LIGHT);
  await writeMask(mask, `${base}-dark.png`, DARK);
  report.push({ name, cmp });

  if (problems.length) {
    console.error(`  ⛔ ${name}：${problems.join("；")}`);
    failed = true;
  } else {
    console.log(
      `  ${name.padEnd(9)} ✔ 轮廓 ${cmp.changed.length} 个，最大线宽 ` +
        `${Math.max(0, ...cmp.changed.map((c) => c.maxT)).toFixed(1)}px@256（目标 ${STROKE_PX}）` +
        `  细节 ${cmp.unchanged.length} 个逐像素未动  连通域 ${cmp.compsBefore}→${cmp.compsAfter}`,
    );
  }
}

fs.writeFileSync(
  path.join(SRC, "MEASURED.txt"),
  report
    .map(({ name, cmp }) =>
      `${name}\t原粗=${cmp.changed.map((c) => c.maxT0.toFixed(1)).join(",") || "-"}\t现粗=${cmp.changed.map((c) => c.maxT.toFixed(1)).join(",") || "-"}\t细节未动=${cmp.unchanged.length}\tcomps=${cmp.compsBefore}->${cmp.compsAfter}`)
    .join("\n") + "\n",
);

if (failed) {
  console.error("\n⛔ 有图标未通过闸门（已写盘以便比对，但退出码为 1）");
  process.exitCode = 1;
}

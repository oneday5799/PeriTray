/**
 * SVG 路径的**矢量法线偏移**。
 *
 * ── 为什么必须走矢量、不能对像素做形态学 ────────────────────────
 *   像素域的做法（腐蚀 / 开运算 / 分水岭脊线）**必然会动外轮廓**：
 *   局部偏薄处整段被删 ⇒ 断口；结构交界处 r 突变 ⇒ 鼓包。
 *   用户实测反馈正是「轮廓变形／有缺口凸起」。
 *   ⇒ 矢量域里「把轮廓整体内移 d」是**逐点精确**的：
 *     直线段偏移后仍是直线，圆弧仍是圆弧，缓弯 bézier 误差 < 0.1 单位。
 *     本仓的偏移量只有 6.4~8.5 单位（1024 画布 ⇒ 0.6%~0.8%），
 *     远小于任何一条曲线的曲率半径 ⇒ **形状逐点保持，只整体变小一点**。
 *
 * ── 为什么孔洞要往**外**推、外轮廓要往**内**收 ────────────────────
 *   描边宽度 = 外轮廓到孔洞的距离。要把 64 变 51.2，
 *   就得「外轮廓内移 6.4 + 孔洞外移 6.4」。
 *   ⚠️ 哪条子路径是孔洞**不用启发式判定**（试过：鼠标的滚动轮
 *      也在外轮廓的包围盒内，会被误判成孔洞），
 *      而是由调用方用**显式表格**逐条指定 —— 可读、可审、可改。
 *
 * 用法：
 *   import { offsetSubpaths } from "./svg-offset.mjs";
 *   const out = offsetSubpaths(d, { radii: { 0: +6.4, 1: -6.4 } });
 */

/**
 * 解析**整条** `d` ⇒ 若干条子路径（每条是绝对坐标下的段数组）。
 *
 * ⛔ 早先按 `/[Mm]/` 把 `d` 切成段再逐段解析，于是**每段子路径都从 (0,0) 起算**。
 *   但后续子路径常用**相对 moveto**（例：键盘内框是 `m0 76.8`，
 *   基准是上一子路径的终点 (0, 140.8)）⇒ 首点被算成 (0, 76.8)
 *   ⇒ 法线错、偏移错 ⇒ 渲染出左上角一条飞出去的尖刺。
 *   ⇒ 必须**整条 `d` 一次解析**、在子路径之间继续维护当前点。
 *
 * @returns {Array<Array<[string, ...]>>}
 */
export function parsePath(text) {
  // 拆成「命令 + 数字」记号
  const tokens = text.match(/[MmLlHhVvCcSsQqTtAaZz]|-?\d*\.?\d+(?:e[-+]?\d+)?/gi) || [];
  const subs = [];
  let cur = null;
  let i = 0;
  let cmd = null;
  let cx = 0, cy = 0;          // 当前点
  let sx = 0, sy = 0;          // 子路径起点
  let prevC2 = null, prevQ = null;   // 上一次的控制点（用于 S/T）
  const num = () => parseFloat(tokens[i++]);

  while (i < tokens.length) {
    if (/^[A-Za-z]$/.test(tokens[i])) { cmd = tokens[i]; i++; }
    else if (cmd === "M") cmd = "L";            // 后续隐式坐标按 L 处理
    else if (cmd === "m") cmd = "l";
    const rel = cmd === cmd.toLowerCase();
    const C = cmd.toUpperCase();

    if (C === "Z") {
      cur.push(["Z"]);
      cx = sx; cy = sy;
      prevC2 = prevQ = null;
      continue;
    }
    if (C === "M") {
      let x = num(), y = num();
      if (rel) { x += cx; y += cy; }
      cur = [["M", x, y]];
      subs.push(cur);
      cx = sx = x; cy = sy = y;
      prevC2 = prevQ = null;
      continue;
    }
    if (C === "L") {
      let x = num(), y = num();
      if (rel) { x += cx; y += cy; }
      cur.push(["L", x, y]);
      cx = x; cy = y;
      prevC2 = prevQ = null;
      continue;
    }
    if (C === "H") {
      let x = num();
      if (rel) x += cx;
      cur.push(["L", x, cy]);
      cx = x;
      prevC2 = prevQ = null;
      continue;
    }
    if (C === "V") {
      let y = num();
      if (rel) y += cy;
      cur.push(["L", cx, y]);
      cy = y;
      prevC2 = prevQ = null;
      continue;
    }
    if (C === "C" || C === "S") {
      let c1x, c1y, c2x, c2y, x, y;
      if (C === "C") {
        c1x = num(); c1y = num(); c2x = num(); c2y = num(); x = num(); y = num();
        if (rel) { c1x += cx; c1y += cy; c2x += cx; c2y += cy; x += cx; y += cy; }
      } else {
        c2x = num(); c2y = num(); x = num(); y = num();
        if (rel) { c2x += cx; c2y += cy; x += cx; y += cy; }
        c1x = prevC2 ? 2 * cx - prevC2[0] : cx;
        c1y = prevC2 ? 2 * cy - prevC2[1] : cy;
      }
      cur.push(["C", c1x, c1y, c2x, c2y, x, y]);
      prevC2 = [c2x, c2y]; prevQ = null;
      cx = x; cy = y;
      continue;
    }
    if (C === "Q" || C === "T") {
      let qx, qy, x, y;
      if (C === "Q") { qx = num(); qy = num(); x = num(); y = num(); }
      else { x = num(); y = num(); qx = prevQ ? 2 * cx - prevQ[0] : cx; qy = prevQ ? 2 * cy - prevQ[1] : cy; }
      if (rel) { qx += cx; qy += cy; x += cx; y += cy; }
      // 升阶为三次
      cur.push([
        "C",
        cx + (2 / 3) * (qx - cx), cy + (2 / 3) * (qy - cy),
        x + (2 / 3) * (qx - x), y + (2 / 3) * (qy - y),
        x, y,
      ]);
      prevQ = [qx, qy]; prevC2 = null;
      cx = x; cy = y;
      continue;
    }
    if (C === "A") {
      const rx = num(), ry = num(), rot = num();
      const laf = num(), sf = num();
      let x = num(), y = num();
      if (rel) { x += cx; y += cy; }
      for (const s of arcToCubics(cx, cy, rx, ry, rot, laf, sf, x, y)) cur.push(s);
      prevC2 = prevQ = null;
      cx = x; cy = y;
      continue;
    }
    throw new Error(`未支持的路径命令：${cmd}`);
  }
  return subs;
}

/** 椭圆弧 → 三次 bézier 段（端点参数化，含大弧与旋转）。 */
function arcToCubics(x1, y1, rx, ry, phiDeg, largeArc, sweep, x2, y2) {
  if (rx === 0 || ry === 0 || (x1 === x2 && y1 === y2)) return [["L", x2, y2]];
  rx = Math.abs(rx); ry = Math.abs(ry);
  const phi = (phiDeg * Math.PI) / 180;
  const cp = Math.cos(phi), sp = Math.sin(phi);
  const dx2 = (x1 - x2) / 2, dy2 = (y1 - y2) / 2;
  const x1p = cp * dx2 + sp * dy2;
  const y1p = -sp * dx2 + cp * dy2;
  let lam = (x1p * x1p) / (rx * rx) + (y1p * y1p) / (ry * ry);
  if (lam > 1) { const s = Math.sqrt(lam); rx *= s; ry *= s; }
  const sign = largeArc === sweep ? -1 : 1;
  const num = rx * rx * ry * ry - rx * rx * y1p * y1p - ry * ry * x1p * x1p;
  const den = rx * rx * y1p * y1p + ry * ry * x1p * x1p;
  const co = sign * Math.sqrt(Math.max(0, num / den));
  const cxp = (co * rx * y1p) / ry;
  const cyp = (-co * ry * x1p) / rx;
  const cx = cp * cxp - sp * cyp + (x1 + x2) / 2;
  const cy = sp * cxp + cp * cyp + (y1 + y2) / 2;
  const ang = (ux, uy, vx, vy) => {
    const dot = ux * vx + uy * vy;
    const len = Math.hypot(ux, uy) * Math.hypot(vx, vy);
    let a = Math.acos(Math.min(1, Math.max(-1, dot / len)));
    if (ux * vy - uy * vx < 0) a = -a;
    return a;
  };
  const th1 = ang(1, 0, (x1p - cxp) / rx, (y1p - cyp) / ry);
  let dth = ang((x1p - cxp) / rx, (y1p - cyp) / ry, (-x1p - cxp) / rx, (-y1p - cyp) / ry);
  if (!sweep && dth > 0) dth -= 2 * Math.PI;
  else if (sweep && dth < 0) dth += 2 * Math.PI;
  const n = Math.max(1, Math.ceil(Math.abs(dth) / (Math.PI / 2)));
  const out = [];
  let px = x1, py = y1;
  for (let k = 1; k <= n; k++) {
    const t0 = th1 + (dth * (k - 1)) / n;
    const t1 = th1 + (dth * k) / n;
    const a = 4 / 3 * Math.tan((t1 - t0) / 4);
    const map = (t) => [
      cx + rx * cp * Math.cos(t) - ry * sp * Math.sin(t),
      cy + rx * sp * Math.cos(t) + ry * cp * Math.sin(t),
    ];
    const d = (t) => [
      -rx * cp * Math.sin(t) - ry * sp * Math.cos(t),
      -rx * sp * Math.sin(t) + ry * cp * Math.cos(t),
    ];
    const [ex, ey] = map(t1);
    const [dx1, dy1] = d(t0);
    const [dx2b, dy2b] = d(t1);
    out.push(["C", px + a * dx1, py + a * dy1, ex - a * dx2b, ey - a * dy2b, ex, ey]);
    px = ex; py = ey;
  }
  return out;
}

/** 有符号面积（shoelace，按曲线采样）。>0 ⇒ 屏幕坐标下顺时针。 */
function signedArea(segs) {
  const pts = flatten(segs, 24);
  let a = 0;
  for (let i = 0; i < pts.length; i++) {
    const [x1, y1] = pts[i];
    const [x2, y2] = pts[(i + 1) % pts.length];
    a += x1 * y2 - x2 * y1;
  }
  return a / 2;
}

/** 采样成折线（用于面积与法线估计）。 */
function flatten(segs, per = 24) {
  const pts = [];
  let cur = null;
  for (const s of segs) {
    if (s[0] === "M") { cur = [s[1], s[2]]; pts.push(cur); }
    else if (s[0] === "L") { cur = [s[1], s[2]]; pts.push(cur); }
    else if (s[0] === "C") {
      const [x0, y0] = cur;
      for (let k = 1; k <= per; k++) {
        const t = k / per, u = 1 - t;
        pts.push([
          u * u * u * x0 + 3 * u * u * t * s[1] + 3 * u * t * t * s[3] + t * t * t * s[5],
          u * u * u * y0 + 3 * u * u * t * s[2] + 3 * u * t * t * s[4] + t * t * t * s[6],
        ]);
      }
      cur = [s[5], s[6]];
    }
  }
  return pts;
}

/**
 * 把一条子路径整体沿法线平移 `d`。
 * @param d > 0 朝**材料内部**（等效外轮廓内收 / 实体缩小）
 *        d < 0 朝**外部**（等效孔洞外扩）
 */
function offsetSubpath(segs, d) {
  if (d === 0) return segs;

  // 每个 on-curve 点的位置索引
  // 带 on-curve 点的段下标（M / L / C 的终点都是 on-curve 点；Z 没有）
  // ⛔ 早先写成 `let k=0; … k += s.length` 是把「段数组的元素个数（M=3、C=7）」
  //    当成了「段序号」⇒ idx 越界 ⇒ pt() 读到 undefined。
  const idx = [];
  for (let i = 0; i < segs.length; i++) {
    if (segs[i][0] === "Z") break;
    idx.push(i);
  }
  const pt = (i) => {
    const s = segs[i];
    return s[0] === "M" || s[0] === "L" ? [s[1], s[2]] : [s[5], s[6]];
  };
  const n = idx.length;
  const normals = [];
  for (let j = 0; j < n; j++) {
    const [ax, ay] = pt(idx[(j - 1 + n) % n]);
    const [bx, by] = pt(idx[(j + 1) % n]);
    let tx = bx - ax, ty = by - ay;
    const len = Math.hypot(tx, ty) || 1;
    tx /= len; ty /= len;
    // ⚠️ 只给**左法线**，方向（内/外）由调用方决定。
    //   ⛔ 不能从绕向推：源文件里各子路径的绕向**并不统一** ——
    //   键盘外框 p0[0] 是正、扬声器外框 p0[1] 是正，但扬声器的**圆环外轮廓
    //   p1[0] 却是负** ⇒ 「按面积符号定方向」会把扬声器的圆环向外扩。
    //   ⇒ 改由 generate-widget-icons 逐子路径**试两个方向、取填充面积更小者**。
    normals.push([ty, -tx]);
  }

  const out = segs.map((s) => s.slice());
  for (let j = 0; j < n; j++) {
    const i = idx[j];
    const [nx, ny] = normals[j];
    const s = out[i];
    if (s[0] === "M" || s[0] === "L") { s[1] += d * nx; s[2] += d * ny; }
    // ⛔ C 段的**终点**同样是 on-curve 点，也必须位移。
    //   早先只处理 M/L ⇒ 11 个 knot 里有 5 个纹丝不动（键盘实测）
    //   ⇒ 描边一边变细一边不变 ⇒ 最大线宽 22.6px（目标 13）。
    else if (s[0] === "C") { s[5] += d * nx; s[6] += d * ny; }
  }
  // 闭合接缝：末尾回到起点的那个点必须与起点**完全一致**，
  // 否则两端法线的中心差分给出不同位移 ⇒ 轮廓在接缝处裂开。
  {
    const first = out[0];
    if (first && first[0] === "M") {
      for (let i = out.length - 1; i >= 1; i--) {
        const t = out[i];
        if (t[0] === "Z") continue;
        const ox = t[0] === "C" ? t[5] : t[1];
        const oy = t[0] === "C" ? t[6] : t[2];
        if (Math.abs(ox - segs[0][1]) < 1e-6 && Math.abs(oy - segs[0][2]) < 1e-6) {
          if (t[0] === "C") { t[5] = first[1]; t[6] = first[2]; }
          else { t[1] = first[1]; t[2] = first[2]; }
        }
        break;
      }
    }
  }
  // 控制点：**c1 用起点法线、c2 用终点法线**。
  // ⛔ 早先两个控制点都用「两端法线平均」—— 键盘的圆角跨度 90°，
  //   两端法线相差 90°，平均后方向偏 45° ⇒ 圆角被**顶出去**
  //   ⇒ 实测键盘最大线宽 72px（目标 13）、扬声器 89px。
  for (let j = 0; j < n; j++) {
    const i = idx[j];
    const s = segs[i];
    if (s[0] !== "C") continue;
    const nOut = normals[j];
    const nIn = normals[(j + 1) % n];
    const o = out[i];
    o[1] += d * nOut[0]; o[2] += d * nOut[1];
    o[3] += d * nIn[0];  o[4] += d * nIn[1];
  }
  // 起点 M 也可能同时是上一段的终点（L 链），上面已按 on-curve 处理
  return out;
}

function fmt(v) {
  const r = Math.round(v * 1000) / 1000;
  return Object.is(r, -0) ? "0" : String(r);
}

function serialize(segs) {
  return segs
    .map((s) => (s[0] === "Z" ? "z" : s[0] + s.slice(1).map(fmt).join(" ")))
    .join("")
    .replace(/([MLC]) -/g, "$1-");
}

/**
 * 对一条 `d` 里的每条子路径按 `radii[i]` 偏移。
 * @param {string} d
 * @param {{radii: Record<number, number>}} opts radii[子路径序号] = 位移量（0 = 不动）
 * @returns {string} 新的 d
 */
export function offsetSubpaths(d, { radii }) {
  return parsePath(d)
    .map((segs, i) => {
      const delta = radii[i] ?? 0;
      return delta === 0 ? serialize(segs) : serialize(offsetSubpath(segs, delta));
    })
    .join("");
}

/** 供调用方核对：某条子路径的有符号面积（判断顺/逆时针）。 */
export function subpathArea(d, i) {
  return signedArea(parsePath(d)[i]);
}

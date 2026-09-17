import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "..");
const DIST = path.join(ROOT, "src-tauri", "dist");
const PAGES = ["popup.html", "settings.html"];

const KEYWORDS = new Set(
  ("if for while switch catch function return var let const new delete typeof void " +
    "in of do else try finally throw class extends super yield await async this " +
    "break continue case default debugger instanceof with").split(" "),
);

const BROWSER_GLOBALS = new Set(
  [
    "document", "window", "console", "setTimeout", "setInterval", "clearTimeout",
    "clearInterval", "requestAnimationFrame", "cancelAnimationFrame", "fetch",
    "Date", "Math", "JSON", "Object", "Array", "String", "Number", "Boolean",
    "Promise", "Error", "TypeError", "RangeError", "parseInt", "parseFloat",
    "isNaN", "encodeURIComponent", "decodeURIComponent", "getComputedStyle",
    "matchMedia", "localStorage", "sessionStorage", "URL", "URLSearchParams",
    "Blob", "FileReader", "CustomEvent", "Event", "MouseEvent", "PointerEvent",
    "KeyboardEvent", "WheelEvent", "InputEvent", "IntersectionObserver",
    "MutationObserver", "ResizeObserver", "alert", "confirm", "prompt",
    "open", "close", "focus", "blur", "scrollTo", "scrollBy", "structuredClone",
    "crypto", "navigator", "location", "history", "screen", "performance",
    "RegExp", "Map", "Set", "Symbol", "Proxy", "Reflect", "Function",
  ],
);

const LOCAL_CALLBACKS = new Set(
  ["resolve", "reject", "get", "set"],
);

const errors = [];

function read(p) {
  return fs.readFileSync(p, "utf8");
}

function stripLiterals(src) {
  let out = "";
  let i = 0;
  const n = src.length;
  while (i < n) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "*") {
      const j = src.indexOf("*/", i + 2);
      i = j < 0 ? n : j + 2;
      continue;
    }
    if (c === "/" && d === "/") {
      const j = src.indexOf("\n", i);
      i = j < 0 ? n : j;
      continue;
    }
    if (c === "'" || c === '"') {
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") j += 2;
        else if (src[j] === c) break;
        else j++;
      }
      out += " ";
      i = j + 1;
      continue;
    }
    if (c === "`") {
      let j = i + 1;
      while (j < n) {
        if (src[j] === "\\") j += 2;
        else if (src[j] === "`") break;
        else j++;
      }
      out += " ";
      i = j + 1;
      continue;
    }
    out += c;
    i++;
  }
  return out;
}

function declaredNames(cleanSrc) {
  const names = new Set();
  for (
    const m of cleanSrc.matchAll(
      /(?:^|\n)\s*(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/g,
    )
  ) names.add(m[1]);
  for (const m of cleanSrc.matchAll(/window\.([A-Za-z_$][\w$]*)\s*=/g)) {
    names.add(m[1]);
  }
  for (
    const m of cleanSrc.matchAll(
      /(?:^|\n)\s*(?:let|const|var)\s+([A-Za-z_$][\w$]*)/g,
    )
  ) names.add(m[1]);
  // 解构声明（const { invoke } = ...）：绑定名计入声明池，
  // 否则 settings 各脚本裸调 invoke 只能靠 common 内部局部变量意外通过审计
  for (
    const m of cleanSrc.matchAll(
      /(?:^|\n)\s*(?:let|const|var)\s*\{([^}]*)\}\s*=/g,
    )
  ) collectParams(m[1], names);
  for (const m of cleanSrc.matchAll(/\bfunction\s*[A-Za-z_$]?[\w$]*\s*\(([^()]*)\)/g)) {
    collectParams(m[1], names);
  }
  for (const m of cleanSrc.matchAll(/([A-Za-z_$][\w$]*)\s*=>/g)) names.add(m[1]);
  for (const m of cleanSrc.matchAll(/\(([^()]*)\)\s*=>/g)) {
    collectParams(m[1], names);
  }
  for (
    const m of cleanSrc.matchAll(
      /[,{]\s*([A-Za-z_$][\w$]*)\s*\([^()]*\)\s*\{/g,
    )
  ) names.add(m[1]);
  // 方法简写的形参计入声明池（含解构与默认值，如 bindHeaderClick(card, { onChanged } = {})），
  // 锚点与方法名规则一致（前置 , 或 {），不会误吞 if/for 等控制结构
  for (
    const m of cleanSrc.matchAll(
      /[,{]\s*[A-Za-z_$][\w$]*\s*\(([^()]*)\)\s*\{/g,
    )
  ) collectParams(m[1], names);
  return names;
}

function collectParams(raw, into) {
  for (const p of raw.split(",")) {
    let t = p.trim();
    // 浅剥外层包裹（解构花括号/数组字面量等），不做嵌套递归
    t = t.replace(/^[{[(]+/, "").replace(/[}\])]$/, "");
    if (!t) continue;
    // 解构重命名 { a: b } 的绑定名是 b；默认值 { a = 1 } 的绑定名是 a
    const renamed = t.includes(":") ? t.split(":").pop() : t;
    const id = renamed.trim().split(/[=\s]/)[0].trim();
    if (/^[A-Za-z_$][\w$]*$/.test(id)) into.add(id);
  }
}

function calledNames(cleanSrc) {
  const out = new Set();
  for (
    const m of cleanSrc.matchAll(/(?<![.\w$"'])([A-Za-z_$][\w$]*)\s*\(/g)
  ) out.add(m[1]);
  return out;
}

function hasBom(p) {
  const b = fs.readFileSync(p);
  return b.length >= 3 && b[0] === 0xef && b[1] === 0xbb && b[2] === 0xbf;
}

const referencedJs = new Set();
const referencedCss = new Set();

for (const page of PAGES) {
  const htmlPath = path.join(DIST, page);
  const html = read(htmlPath);
  const dir = path.dirname(htmlPath);

  for (const m of html.matchAll(/<link[^>]+href="([^"]+\.css)"/g)) {
    const abs = path.resolve(dir, m[1]);
    referencedCss.add(abs);
    if (!fs.existsSync(abs)) errors.push(`${page} 引用不存在的样式: ${m[1]}`);
  }

  const pageJs = [];
  for (const m of html.matchAll(/<script[^>]+src="([^"]+\.js)"/g)) {
    const abs = path.resolve(dir, m[1]);
    referencedJs.add(abs);
    if (!fs.existsSync(abs)) {
      errors.push(`${page} 引用不存在的脚本: ${m[1]}`);
      continue;
    }
    pageJs.push(abs);
  }

  const defined = new Set();
  for (const f of pageJs) {
    for (const n of declaredNames(stripLiterals(read(f)))) defined.add(n);
  }

  // ── 跨文件同名全局函数检测 ──────────────────────────────
  // 经典脚本共享全局作用域，同名顶层 function 声明会互相遮蔽（后加载者
  // 覆盖先加载者），属静默逻辑错误。判例：updateDeviceCard 在音量页与
  // 设备信息页同名，volume-changed 事件误调设备信息版函数致滑块永不更新。
  const fnOwners = new Map();
  for (const f of pageJs) {
    const clean = stripLiterals(read(f));
    for (const m of clean.matchAll(/^(?:async\s+)?function\s+([A-Za-z_$][\w$]*)/gm)) {
      const name = m[1];
      if (!fnOwners.has(name)) fnOwners.set(name, []);
      fnOwners.get(name).push(path.basename(f));
    }
  }
  for (const [name, files] of fnOwners) {
    if (files.length > 1) {
      errors.push(
        `${page}: 同名全局函数 "${name}" 在 ${files.join("、")} 中重复定义（后加载会遮蔽前加载）`,
      );
    }
  }

  for (const f of pageJs) {
    const clean = stripLiterals(read(f));
    for (const name of calledNames(clean)) {
      if (
        !KEYWORDS.has(name) &&
        !BROWSER_GLOBALS.has(name) &&
        !LOCAL_CALLBACKS.has(name) &&
        !defined.has(name) &&
        !(name in globalThis)
      ) {
        errors.push(`${page}: ${path.basename(f)} 调用了未定义的 "${name}"`);
      }
    }
  }
}

for (const dirName of ["scripts", "styles"]) {
  const dirPath = path.join(DIST, dirName);
  const refSet = dirName === "scripts" ? referencedJs : referencedCss;
  for (const f of fs.readdirSync(dirPath)) {
    const abs = path.join(dirPath, f);
    if (!refSet.has(abs)) {
      errors.push(`孤立文件(未被任何页面引用): ${dirName}/${f}`);
    }
  }
}

const bomTargets = [PAGES.map((p) => path.join(DIST, p))].flat();
for (const dirName of ["styles", "scripts"]) {
  for (const f of fs.readdirSync(path.join(DIST, dirName))) {
    bomTargets.push(path.join(DIST, dirName, f));
  }
}
for (const p of bomTargets) {
  if (hasBom(p)) errors.push(`BOM: ${path.relative(ROOT, p)}`);
}

for (const dirName of ["scripts"]) {
  for (const f of fs.readdirSync(path.join(DIST, dirName))) {
    if (!f.endsWith(".js")) continue;
    try {
      execFileSync(process.execPath, ["--check", path.join(DIST, dirName, f)], {
        stdio: "pipe",
      });
    } catch (e) {
      errors.push(`语法错误 ${dirName}/${f}: ${e.stderr?.toString().split("\n")[0]}`);
    }
  }
}

// ── Toast 文案不得含 HTML 标签（P1-6）──────────────────────
// `showToast` 用 `textContent` 写入（防 XSS，必须保持），所以文案里的 HTML 换行标签
// 会被原样显示成字面量。换行请写 `\n`，由 `.toast` 的 `white-space: pre-line` 渲染。
//
// 只扫 `showToast(...)` 的**实参文本**，不误伤页面里合法拼装的 innerHTML / 内联 SVG。
// 括注配对时会跳过字符串与注释，故多行调用、实参里含 `(` `)` 都能正确取到边界。
//
// 定位调用起点前先屏蔽注释：否则注释掉的 showToast 调用（实参里带 HTML 标签）会被当成真调用
// 而误报（这类误报会让闸门被开发者忽略，比漏报更糟）。屏蔽时保持字符偏移量不变，
// 实参仍从**原文**切片，故字符串内容不会丢。
function maskComments(src) {
  const out = src.split("");
  let i = 0;
  while (i < src.length) {
    const c = src[i];
    const d = src[i + 1];
    if (c === "/" && d === "*") {
      const j = src.indexOf("*/", i + 2);
      const e = j < 0 ? src.length : j + 2;
      for (let k = i; k < e; k++) if (out[k] !== "\n") out[k] = " ";
      i = e;
      continue;
    }
    if (c === "/" && d === "/") {
      let j = i;
      while (j < src.length && src[j] !== "\n") j++;
      for (let k = i; k < j; k++) out[k] = " ";
      i = j;
      continue;
    }
    if (c === "'" || c === '"' || c === "`") {
      let j = i + 1;
      while (j < src.length) {
        if (src[j] === "\\") j += 2;
        else if (src[j] === c) break;
        else j++;
      }
      i = j + 1;
      continue;
    }
    i++;
  }
  return out.join("");
}

function toastCallArgs(src) {
  const args = [];
  const masked = maskComments(src);
  const re = /\bshowToast\s*\(/g;
  let m;
  while ((m = re.exec(masked))) {
    const start = m.index + m[0].length;
    let i = start;
    let depth = 1;
    while (i < src.length && depth > 0) {
      const c = src[i];
      const d = src[i + 1];
      if (c === "/" && d === "*") {
        const j = src.indexOf("*/", i + 2);
        i = j < 0 ? src.length : j + 2;
        continue;
      }
      if (c === "/" && d === "/") {
        const j = src.indexOf("\n", i);
        i = j < 0 ? src.length : j;
        continue;
      }
      if (c === "'" || c === '"' || c === "`") {
        let j = i + 1;
        while (j < src.length) {
          if (src[j] === "\\") j += 2;
          else if (src[j] === c) break;
          else j++;
        }
        i = j + 1;
        continue;
      }
      if (c === "(") depth++;
      else if (c === ")") {
        depth--;
        if (depth === 0) break;
      }
      i++;
    }
    args.push(src.slice(start, i));
    re.lastIndex = i; // 跳过已扫描区间，避免同一调用被重复计入
  }
  return args;
}

for (const f of fs.readdirSync(path.join(DIST, "scripts"))) {
  if (!f.endsWith(".js")) continue;
  const src = read(path.join(DIST, "scripts", f));
  for (const argText of toastCallArgs(src)) {
    const tag = argText.match(/<\/?[a-zA-Z][a-zA-Z0-9]*\s*\/?>/);
    if (tag) {
      errors.push(
        `scripts/${f}: showToast 文案含 HTML 标签 ${tag[0]}` +
          `（showToast 走 textContent，会显示成字面量；换行请用 \\n）`,
      );
    }
  }
}

// ── `.toast` 必须能渲染换行（P1-6 的另一半）────────────────
// showToast 的文案是纯文本（textContent），换行靠 `\n` + `white-space: pre-line`。
// 少了这条样式，上面那批 `\n` 会退化成空格、两行提示挤成一行——
// 与「文案里写 HTML 换行标签」是同一个 bug 的两面，故必须成对守住。
{
  const css = read(path.join(DIST, "styles", "base.css"));
  // 注意 `.toast` 会同时出现在两处：① 与 tooltip 共用的「flyout surface」选择器组
  // （组的最后一行正好是 `.toast {`，故也会被匹配到，且它确实作用于 .toast）；
  // ② 独立的 `.toast` 规则块。同特异性下后者覆盖前者，故按出现顺序取
  // **最后一次声明的 white-space 值**——这就是该属性的层叠结果。
  const rules = [...css.matchAll(/^\.toast\s*\{([^}]*)\}/gm)];
  let effective = null;
  for (const r of rules) {
    const decl = r[1].match(/white-space\s*:\s*([^;]+);/);
    if (decl) effective = decl[1].trim();
  }
  if (effective !== "pre-line") {
    errors.push(
      `base.css: .toast 的 white-space 应为 pre-line，实为 ${effective ?? "(未声明)"}` +
        `（toast 文案里的 \\n 将不换行）`,
    );
  }
}

// ── 版本号一致性：五处须为同一版本 ─────────────────────────
// tauri.conf.json / Cargo.toml [package] / Cargo.lock(PeriTray) /
// package.json / settings.html 关于页占位文案
{
  const versions = {
    "tauri.conf.json": (() => {
      try { return JSON.parse(read(path.join(ROOT, "src-tauri", "tauri.conf.json"))).version; }
      catch { return undefined; }
    })(),
    "Cargo.toml": read(path.join(ROOT, "src-tauri", "Cargo.toml"))
      .match(/^version\s*=\s*"([^"]+)"/m)?.[1],
    "Cargo.lock": read(path.join(ROOT, "src-tauri", "Cargo.lock"))
      .match(/name = "PeriTray"\s*\nversion = "([^"]+)"/)?.[1],
    "package.json": (() => {
      try { return JSON.parse(read(path.join(ROOT, "package.json"))).version; }
      catch { return undefined; }
    })(),
    "settings.html": read(path.join(DIST, "settings.html"))
      .match(/版本 v(\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?)/)?.[1],
  };
  const bad = Object.values(versions).some((v) => !v);
  if (new Set(Object.values(versions)).size > 1 || bad) {
    const detail = Object.entries(versions)
      .map(([k, v]) => `${k}=${v ?? "?"}`)
      .join(", ");
    errors.push(`版本号不一致（五处需同步）: ${detail}`);
  }
}

if (errors.length) {
  console.error("检查失败:");
  for (const e of errors) console.error("  ✗ " + e);
  process.exit(1);
}
console.log("前端完整性检查通过");

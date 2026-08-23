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
  return names;
}

function collectParams(raw, into) {
  for (const p of raw.split(",")) {
    const id = p.trim().split(/[=\s]/)[0];
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

if (errors.length) {
  console.error("检查失败:");
  for (const e of errors) console.error("  ✗ " + e);
  process.exit(1);
}
console.log("前端完整性检查通过");

/**
 * L1 / L2 验收脚本 —— 「裸访问 window.__TAURI__」的收敛
 *
 * 运行：
 *   node tools/verify-l1l2.mjs                  # 正常验收（期望全 PASS）
 *   node tools/verify-l1l2.mjs --inject-broken=l1  # 可证伪：把 L1 的判空守卫改回
 *                                                  # 旧形态 `getInvoke()("get_config")`，
 *                                                  # 期望 H 组转红，跑完自动还原
 *   node tools/verify-l1l2.mjs --inject-broken=l2  # 可证伪：删掉一条 onTauriEvent 注册，
 *                                                  # 期望 G1 转红，跑完自动还原
 *
 * ── 改了什么 ──────────────────────────────────────────────────
 *   L1  `popup-audio.js` 的 config-changed 处理器里，
 *       `cfg = await getInvoke()("get_config")` 是全仓**唯一**不判空就直接调用的
 *       `getInvoke()` 调用点。`getInvoke()` 的契约（其定义 + 其余 15 处调用点）
 *       是「可能返回 null」，所以这处是靠「处理器能跑 ⇒ core 一定在」这个隐式假设
 *       在工作——而 P2-3 那次改造的全部意义就是消除这类隐式假设（部分注入：
 *       `event` 在、`core` 不在）。
 *       ⚠️ 如实说明：旧形态**不会崩溃**，TypeError 被处理器自身的 try/catch 吃掉，
 *       唯一可观测差异是**多打一条误导性的 "Failed to reload mute lock config"**。
 *       本条价值在于「契约一致 + 少一条误导日志」，不是「修了一个崩溃」。
 *   L2  3 个文件 5 处裸 `window.__TAURI__.event.listen(...)` 统一改走
 *       `common.js` 的 `onTauriEvent()`（就绪检查在内、未就绪静默跳过并返回 false），
 *       并删掉外层各自的 `if (window.__TAURI__ && window.__TAURI__.event)` 包装。
 *       保留 `popup.js` 的 `if (window.__TAURI__)`：它是「运行时是否注入」的
 *       **捕获载体**（分支 + 特性检测，决定首屏走水合还是 DOMContentLoaded 兜底），
 *       不是裸事件注册。
 *
 * ── 为什么必须跑运行时，不能只 grep ────────────────────────────
 * `tools/check.mjs` 把「未加守卫的 API 访问」明确列为**它看不见的类别**。
 * grep 只能证明「旧写法没了」，证明不了「事件还注册得上」——删 wrapper 时
 * 顺手删掉一行 `listen`，grep 与 `node --check` 全绿。故本脚本两段都要：
 *   · A 组（Node 侧静态）：裸访问的**白名单收敛**，逐行判定；
 *   · G 组（浏览器侧运行时）：无头 Edge 加载**真实 popup.html**，
 *     桩记录每一次 `listen`，断言「该注册的**一个不少**」。
 *   · H 组：L1 的判空守卫，含**正控**（见下）。
 *
 * ── 反恒真的两处关键设计 ──────────────────────────────────────
 *   1. G1 用**逐事件计数**而不是「至少有监听」。旧写法在 `__TAURI__` 存在时也能
 *      注册成功，所以「有没有监听」区分不了新旧；**少一条**才是真回归。
 *   2. H 组带**正控 H4**：恢复 `core` 并让桩返回 `mute_lock: true`，断言处理器
 *      确实去拉了 config 并生效。否则 H1/H2「不抛错、不打日志」可能只是因为
 *      处理器压根没走那条分支——那就是恒真。
 *
 * ── 副作用与清理 ──────────────────────────────────────────────
 * 探针 HTML 必须写在 `dist/` 下（脚本用相对路径引用），名 `__probe_l1l2.html`，
 * `finally` 中删除；`--inject-broken=*` 会临时改写对应源文件，同样在 `finally`
 * 中还原。运行后用 `git status --porcelain` 确认无残留。
 *
 * ⚠️ 这是一次性验收实用脚本（与 L1/L2 一起归档），**不是**每次提交都跑的回归闸门。
 * 它按当时的注册面**硬编码**了期望集合；日后正常新增事件监听时，请同步 EXPECTED
 * 而不是把它当 CI 用。若需要常规回归，应改为独立测试框架下的用例。
 */
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "..");
const DIST = path.join(ROOT, "src-tauri", "dist");
const SCRIPTS = path.join(DIST, "scripts");
const AUDIO_JS = path.join(SCRIPTS, "popup-audio.js");
const DEVICES_JS = path.join(SCRIPTS, "popup-devices.js");
const PROBE = path.join(DIST, "__probe_l1l2.html");

const EDGE_CANDIDATES = [
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
];

const injectArg = process.argv.find((a) => a.startsWith("--inject-broken"));
const INJECT = injectArg ? (injectArg.split("=")[1] || "l1") : null;
if (INJECT && INJECT !== "l1" && INJECT !== "l2") {
  console.error(`✗ 未知的注入目标：${INJECT}（只支持 l1 / l2）`);
  process.exit(2);
}

// ── 先注入、后断言：A 组必须看到**被改坏**的源码 ──────────────────
// （本脚本第一版把 A 组放在注入之前，结果注入 l1 后 A1 仍报 PASS——
//  「静态判据能抓住裸访问」这件事就没被证明。顺序即判据的一部分。）
const originalAudio = fs.readFileSync(AUDIO_JS, "utf8");
const originalDevices = fs.readFileSync(DEVICES_JS, "utf8");
let injected = null;

const edge = EDGE_CANDIDATES.find((p) => fs.existsSync(p));
if (!edge) {
  console.error("✗ 未找到 msedge.exe");
  process.exit(2);
}

function applyInjection() {
  if (INJECT === "l1") {
    // 还原成旧形态：不判空，直接当函数调用
    const broken = originalAudio.replace(
      /const fn = getInvoke\(\);[\s\S]*?cfg = await fn\("get_config"\);/,
      'cfg = await getInvoke()("get_config");'
    );
    if (broken === originalAudio) {
      console.error("✗ 注入失败：未匹配到 L1 守卫（脚本与源码已漂移，请先修脚本）");
      process.exit(2);
    }
    fs.writeFileSync(AUDIO_JS, broken);
    injected = "l1";
  } else if (INJECT === "l2") {
    // 删掉一条注册（模拟「删 wrapper 时顺手删掉一行 listen」）
    // ⚠️ 本仓源码是 CRLF 行尾，正则必须写成 `\r?\n`，否则永远匹配不到。
    const broken = originalDevices.replace(
      /onTauriEvent\("bt-battery-updated", scheduleSilentRefresh\);\r?\n/,
      ""
    );
    if (broken === originalDevices) {
      console.error("✗ 注入失败：未匹配到 bt-battery-updated 注册行");
      process.exit(2);
    }
    fs.writeFileSync(DEVICES_JS, broken);
    injected = "l2";
  }
}

function restoreInjection() {
  if (injected === "l1") {
    fs.writeFileSync(AUDIO_JS, originalAudio);
    console.log("（已还原 popup-audio.js 的注入）");
  } else if (injected === "l2") {
    fs.writeFileSync(DEVICES_JS, originalDevices);
    console.log("（已还原 popup-devices.js 的注入）");
  }
  injected = null;
}

// 兜底：正常路径靠下方 finally 还原，这里再挂一层，避免任何早退路径留下注入。
process.on("exit", () => {
  if (injected) {
    try {
      restoreInjection();
    } catch (e) {
      /* exit 阶段不再抛 */
    }
  }
});

applyInjection();

// ── A 组：静态断言（Node 侧） ────────────────────────────────────
const staticResults = [];
function staticOk(name, cond, detail) {
  staticResults.push((cond ? "PASS" : "FAIL") + " | " + name + (detail ? " | " + detail : ""));
}

const jsFiles = fs
  .readdirSync(SCRIPTS)
  .filter((f) => f.endsWith(".js"))
  .sort();

function readLines(file) {
  return fs.readFileSync(path.join(SCRIPTS, file), "utf8").split(/\r?\n/);
}

// A1：全仓不得再有「不判空就直接调用 getInvoke()」
{
  const hits = [];
  for (const f of jsFiles) {
    readLines(f).forEach((line, i) => {
      if (line.includes("getInvoke()(")) hits.push(`${f}:${i + 1}`);
    });
  }
  staticOk("A1 无 `getInvoke()(...)` 直接调用点", hits.length === 0, hits.join(", ") || "0 处");
}

// A2：非 common.js 的文件里，出现 window.__TAURI__ 的行**只允许是注释**。
//     ⚠️ 收紧过程：本脚本第一版还允许「`if (window.__TAURI__) {` 捕获载体」，
//     后来把该载体改为问抽象层（`if (getInvoke())`），于是规则收紧到「零代码」。
{
  const bad = [];
  const allowed = [];
  for (const f of jsFiles) {
    if (f === "common.js") continue; // 抽象层自身，见 A3
    readLines(f).forEach((line, i) => {
      if (!line.includes("__TAURI__")) return;
      const t = line.trim();
      const isComment = t.startsWith("//") || t.startsWith("*") || t.startsWith("/*");
      if (isComment) allowed.push(`${f}:${i + 1}`);
      else bad.push(`${f}:${i + 1}: ${t}`);
    });
  }
  staticOk(
    "A2 非 common.js 文件中裸访问为 0（只剩注释）",
    bad.length === 0,
    bad.join(" ⏎ ") || `仅注释 ${allowed.length} 行：${allowed.join(", ")}`
  );
}

// A2b：首屏分支的「捕获载体」必须问抽象层，而不是直接读 window.__TAURI__
{
  const src = fs.readFileSync(path.join(SCRIPTS, "popup.js"), "utf8");
  staticOk(
    "A2b popup.js 首屏载体走 getInvoke()（能力检测），不是裸读 __TAURI__",
    /if \(getInvoke\(\)\) \{/.test(src)
  );
}

// A3：common.js 必须是唯一的实现层（invoke 包装 + onTauriEvent 都定义在这里）
{
  const src = fs.readFileSync(path.join(SCRIPTS, "common.js"), "utf8");
  staticOk(
    "A3 抽象层唯一：invoke 包装 + onTauriEvent 均在 common.js",
    src.includes("const invoke = (...args) =>") && src.includes("function onTauriEvent(name, handler)")
  );
  const rawListen = [];
  for (const f of jsFiles) {
    if (f === "common.js") continue;
    readLines(f).forEach((line, i) => {
      if (/\.event\.listen\(/.test(line)) rawListen.push(`${f}:${i + 1}`);
    });
  }
  staticOk("A3b 无裸 `.event.listen(` 残留（common.js 外）", rawListen.length === 0, rawListen.join(", ") || "0 处");
}

// A4：三份头注释的依赖行必须已同步（列了 onTauriEvent、不再声称依赖 window.__TAURI__.event）
{
  const heads = {};
  for (const f of ["popup-audio.js", "popup.js", "popup-devices.js"]) {
    heads[f] = readLines(f).slice(0, 10).join("\n");
  }
  staticOk(
    "A4a popup-audio.js 头注释依赖行含 onTauriEvent",
    /依赖：common\.js\([^)]*onTauriEvent/.test(heads["popup-audio.js"])
  );
  staticOk(
    "A4b popup.js 头注释依赖行含 onTauriEvent",
    /依赖：common\.js\([^)]*onTauriEvent/.test(heads["popup.js"])
  );
  staticOk(
    "A4c popup-devices.js 头注释不再声称依赖 window.__TAURI__.event",
    /依赖：common\.js（[^）]*onTauriEvent/.test(heads["popup-devices.js"]) &&
      !/^\s*\*\s*window\.__TAURI__\.event/m.test(heads["popup-devices.js"])
  );
}

// ── 浏览器侧：早于所有脚本执行的 Tauri 桩 ────────────────────────
const STUB = `<script>
(function () {
  var BASE = {
    auto_start: false, hidden_devices: [], hidden_groups: [],
    device_names: {}, device_groups: {}, use_system_bt: false,
    tray_devices: [], hidden_audio_devices: [], mute_lock: false,
    volume_fine_adjust: false, force_mute_devices: [], enable_spatial_sound: false,
    simplify_device_names: true, theme_mode: "follow_system", window_material: "default",
    default_popup_tab: "devices", device_shortcuts: {}, low_battery_devices: []
  };
  // handlers: 事件名 -> 回调数组（**数组**，因为 config-changed 会被 common.js
  // 与 popup-audio.js 各注册一次，后注册的不能把先注册的覆盖掉）
  window.__L = {
    base: BASE,
    backend: JSON.parse(JSON.stringify(BASE)),
    handlers: {},
    order: [],
    calls: [],
    errors: []
  };
  window.addEventListener("error", function (e) {
    window.__L.errors.push(String(e.message));
  });
  window.addEventListener("unhandledrejection", function (e) {
    window.__L.errors.push("unhandled: " + String(e.reason && e.reason.message ? e.reason.message : e.reason));
  });
  window.__TAURI__ = {
    core: {
      invoke: function (cmd) {
        window.__L.calls.push(cmd);
        switch (cmd) {
          case "get_config":
            return Promise.resolve(JSON.parse(JSON.stringify(window.__L.backend)));
          case "get_devices": case "get_devices_fresh":
          case "get_audio_devices": case "get_audio_sessions":
            return Promise.resolve([]);
          default:
            return Promise.resolve(null);
        }
      }
    },
    event: {
      listen: function (name, handler) {
        if (!window.__L.handlers[name]) window.__L.handlers[name] = [];
        window.__L.handlers[name].push(handler);
        window.__L.order.push(name);
        return Promise.resolve(function () {});
      }
    }
  };
})();
</script>`;

// ── 浏览器侧：页面末尾的断言脚本 ─────────────────────────────────
// ⚠️ 本段会被塞进 HTML，避免反引号与 ${ 以免与 Node 模板字符串冲突。
const EXPECTED = {
  "config-changed": 2, // common.js + popup-audio.js
  "material-changed": 1,
  "shortcut-register-failed": 1,
  "update-available": 1,
  "volume-changed": 1,
  "audio-devices-changed": 1,
  "24g-battery-updated": 1,
  "bt-battery-updated": 1,
  "devices-changed": 1,
  "switch-tab": 1,
};

const ASSERTIONS = `<pre id="__l_out"></pre>
<script>
(async function () {
  var R = [];
  function ok(name, cond, detail) {
    R.push((cond ? "PASS" : "FAIL") + " | " + name + (detail ? " | " + detail : ""));
  }
  function info(msg) { R.push("INFO | " + msg); }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  function count(name) {
    var a = window.__L.handlers[name];
    return a ? a.length : 0;
  }
  function srcOf(fn) { return typeof fn === "function" ? fn.toString() : ""; }

  try {
    // 等首屏异步初始化收敛（注册是同步发生的，等一等只为让潜在的同步抛错先暴露）
    await sleep(600);

    // ── G 组：L2 —— 事件注册面必须一个不少 ──
    var EXPECTED = ${JSON.stringify(EXPECTED)};
    var missing = [];
    for (var name in EXPECTED) {
      if (count(name) !== EXPECTED[name]) {
        missing.push(name + "(期望 " + EXPECTED[name] + " 实得 " + count(name) + ")");
      }
    }
    ok("G1 预期事件全部按次数注册（逐事件计数，非「至少有一个」）",
       missing.length === 0,
       missing.length ? missing.join(", ") : Object.keys(EXPECTED).length + " 个事件名全中");

    var extra = Object.keys(window.__L.handlers).filter(function (n) { return !(n in EXPECTED); });
    if (extra.length) info("多出未预期的事件注册（不计失败）：" + extra.join(", "));

    ok("G2 config-changed 有两个登记（common.js + popup-audio.js，未被覆盖）",
       count("config-changed") === 2, "count=" + count("config-changed"));

    var audioCfg = (window.__L.handlers["config-changed"] || []).filter(function (h) {
      return srcOf(h).indexOf("applyAudioRuntimeConfig") >= 0;
    });
    ok("G3 popup-audio.js 的 config-changed 处理器确实登记了", audioCfg.length === 1,
       "匹配 " + audioCfg.length + " 个");

    var sw = window.__L.handlers["switch-tab"] || [];
    ok("G4 switch-tab 走 onTauriEvent 且回调调 switchToTab",
       sw.length === 1 && srcOf(sw[0]).indexOf("switchToTab") >= 0);

    var silentOk = ["24g-battery-updated", "bt-battery-updated", "devices-changed"].every(function (n) {
      var a = window.__L.handlers[n] || [];
      return a.length === 1 && a[0] === scheduleSilentRefresh; // 同一函数引用，未被包装
    });
    ok("G5 三个电量/设备推送的回调仍是 scheduleSilentRefresh 本体（未被包一层）", silentOk);

    // G6：首屏分支的载体（popup.js 的 if (getInvoke())）确实走了 Tauri 侧。
    // 判据用 data-theme 而不是 get_devices：DOMContentLoaded 兜底分支也会在
    // 100ms 后调 loadDevices，所以「有没有 get_devices」区分不了两侧；
    // 而 initTheme() 只在 Tauri 侧被调用，它会设置 <html data-theme>。
    var themeAttr = document.documentElement.getAttribute("data-theme");
    ok("G6 首屏走了 Tauri 侧（initTheme 已设 data-theme，兜底分支不会）",
       themeAttr === "light" || themeAttr === "dark",
       "data-theme=" + String(themeAttr) + ", calls=[" + window.__L.calls.join(",") + "]");

    if (window.__L.errors.length) info("页面运行期报错：" + window.__L.errors.join(" / "));

    // ── H 组：L1 —— getInvoke() 未就绪时的判空守卫 ──
    if (audioCfg.length !== 1) {
      R.push("FAIL | H 组无法执行 | 未定位到 popup-audio.js 的 config-changed 处理器");
    } else {
      var handler = audioCfg[0];
      var savedCore = window.__TAURI__.core;

      // 前置：制造「部分注入」——event 在、core 不在
      delete window.__TAURI__.core;
      ok("H0 前置成立：getInvoke() 此刻返回 null（否则 H1/H2 是恒真）",
         getInvoke() === null, "getInvoke()=" + String(getInvoke()));

      // 直接演示旧形态的危害：不判空就调用确实会抛
      var preThrew = false;
      try { var f = getInvoke(); await f("get_config"); } catch (e) { preThrew = true; }
      ok("H0b 旧形态（不判空直接调用）确实抛错 ⇒ 守卫非多余", preThrew);

      var errs = [];
      var origErr = console.error;
      console.error = function () {
        errs.push(Array.prototype.slice.call(arguments).join(" "));
      };
      var threw = false;
      try { await handler({ payload: null }); } catch (e) { threw = true; }
      console.error = origErr;

      ok("H1 无 payload 且 invoke 未就绪 ⇒ 不抛错", !threw);
      ok("H2 不产生误导性错误日志（旧形态必打 Failed to reload mute lock config）",
         errs.length === 0, errs.length ? errs.join(" | ") : "0 条");

      // ── H4 正控：证明这条分支真的会被走到（否则 H1/H2 恒真）──
      window.__TAURI__.core = savedCore;
      window.__L.backend = JSON.parse(JSON.stringify(window.__L.base));
      window.__L.backend.mute_lock = true;
      var okCtrl = false;
      try {
        await handler({ payload: null });
        okCtrl = muteLockEnabled === true;
      } catch (e) { info("正控抛错：" + e.message); }
      ok("H4 正控：core 就绪 + 空 payload ⇒ 确实去拉了 config 并生效", okCtrl,
         "muteLockEnabled=" + String(muteLockEnabled));

      // ── H5 负控：payload 存在时不得再调 get_config（不应无谓拉取）──
      window.__L.backend = JSON.parse(JSON.stringify(window.__L.base));
      window.__L.backend.mute_lock = false;
      await handler({ payload: JSON.parse(JSON.stringify(window.__L.base)) });
      ok("H5 负控：payload 存在时直接用 payload（mute_lock 回到 false）",
         muteLockEnabled === false, "muteLockEnabled=" + String(muteLockEnabled));
    }
  } catch (e) {
    R.push("FAIL | 断言脚本自身抛错 | " + (e && e.message ? e.message : String(e)));
  }

  document.getElementById("__l_out").textContent = "\\n" + R.join("\\n") + "\\n";
  document.title = R.some(function (l) { return l.indexOf("FAIL") === 0; }) ? "L-RED" : "L-GREEN";
})();
</script>`;

// ── 生成探针 HTML ────────────────────────────────────────────────
const html = fs.readFileSync(path.join(DIST, "popup.html"), "utf8");
const anchor = '<script src="scripts/common.js"></script>';
if (!html.includes(anchor)) {
  console.error(`✗ 未在 popup.html 中找到锚点：${anchor}`);
  process.exit(2);
}
const probeHtml = html
  .replace(anchor, `${STUB}\n  ${anchor}`)
  .replace("</body>", `${ASSERTIONS}\n</body>`);

// ── 跑探针并报告（注入已在文件上方完成，A 组也已按注入后的源码判定）──
let exitCode = 0;
try {
  fs.writeFileSync(PROBE, probeHtml);
  const out = execFileSync(
    edge,
    [
      "--headless=new",
      "--disable-gpu",
      "--no-sandbox",
      "--allow-file-access-from-files",
      "--virtual-time-budget=8000",
      "--dump-dom",
      "file:///" + PROBE.replace(/\\/g, "/"),
    ],
    { encoding: "utf8", maxBuffer: 64 * 1024 * 1024, stdio: ["ignore", "pipe", "pipe"] }
  );

  const m = out.match(/<pre id="__l_out">([\s\S]*?)<\/pre>/);
  if (!m) {
    console.error("✗ 未取到断言输出（探针未执行？）。DOM 片段：");
    console.error(out.slice(0, 1200));
    process.exit(2);
  }
  const lines = m[1]
    .replace(/&quot;/g, '"')
    .replace(/&amp;/g, "&")
    .replace(/&lt;/g, "<")
    .replace(/&gt;/g, ">")
    .split("\n")
    .map((l) => l.trim())
    .filter(Boolean);

  const mode = INJECT ? `可证伪注入模式（--inject-broken=${INJECT}：期望对应组转红）` : "正常模式：期望全 PASS";
  console.log(`── L1/L2 验收（${mode}）──`);
  console.log("  【A 组·静态】");
  for (const l of staticResults) console.log("    " + l);
  console.log("  【G/H 组·运行时】");
  for (const l of lines) console.log("    " + l);

  const all = [...staticResults, ...lines];
  const failed = all.filter((l) => l.startsWith("FAIL"));

  if (INJECT === "l1") {
    const hRed = lines.filter((l) => /^(PASS|FAIL) \| H\d/.test(l) && l.startsWith("FAIL"));
    if (!hRed.length) {
      console.error("\n✗ 注入 l1 后 H 组仍全绿 ⇒ 判据恒真、证明不了任何事");
      exitCode = 1;
    } else {
      console.log(`\n✓ 可证伪成立：注入 l1 后 H 组有 ${hRed.length} 条转红`);
      for (const l of hRed) console.log("    " + l);
    }
  } else if (INJECT === "l2") {
    const g1 = lines.find((l) => /^(PASS|FAIL) \| G1 /.test(l));
    if (!g1 || !g1.startsWith("FAIL")) {
      console.error("\n✗ 注入 l2 后 G1 仍绿 ⇒ 逐事件计数判据没能发现「少一条注册」");
      exitCode = 1;
    } else {
      console.log("\n✓ 可证伪成立：注入 l2 后 G1 转红");
      console.log("    " + g1);
    }
  } else {
    if (failed.length) {
      console.error(`\n✗ ${failed.length} 条断言失败`);
      exitCode = 1;
    } else {
      console.log("\n✓ 全部断言通过");
    }
  }
} finally {
  restoreInjection();
  if (fs.existsSync(PROBE)) fs.unlinkSync(PROBE);
}
process.exit(exitCode);

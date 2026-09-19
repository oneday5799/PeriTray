/**
 * B14 验收脚本 —— 「后端快照是否吞掉本页未落盘的改动」
 *
 * 运行：
 *   node tools/verify-b14.mjs                  # 正常验收（期望全 PASS）
 *   node tools/verify-b14.mjs --inject-broken  # 可证伪：临时把调用点的
 *                                              # `keepLocalEdits: true` 改成 `false`，
 *                                              # 期望 A 组**转红**，跑完自动还原
 *
 * ── 缺陷面比原计划描述的大（本脚本的实测发现）──────────────────
 * 计划只点名 `settings.js` 的 `config-changed` 处理器。实测发现
 * `acceptConfig(await invoke("get_config"))` 共 **8 处**（1 处首份快照 + 7 处
 * 「改完后端某字段再重新拉取」），后 7 处同样会吞掉用户在**别处**的未落盘改动；
 * 而且 `config-changed` 处理器自己就会经 `loadDevicesAsync()` 走到其中一处，
 * **所以只修处理器那一处是无效的**（本脚本第一版就抓到了这个：同步阶段正确、
 * `await` 之后被二次覆盖）。
 *
 * ── 为什么这样测 ──────────────────────────────────────────────
 * B14 是**前端运行时逻辑**缺陷（`tools/check.mjs` 明确声明看不见这一类），
 * 没有 JS 测试框架可用，故用**无头 Edge 加载真实页面**：
 *   1. 在真实 `settings.html` 的**所有脚本之前**注入一个 `window.__TAURI__` 桩，
 *      使 `onTauriEvent()` 把**真实**的 `config-changed` 回调登记进桩里
 *      （`common.js` 的 `onTauriEvent` 直接读 `window.__TAURI__.event.listen`，
 *      所以桩必须早于脚本执行）；
 *   2. 断言脚本在页面末尾，通过 `window.__B14.handlers["config-changed"]`
 *      **调用真实回调**，而不是调用一份复制的实现。
 *   3. 桩的 `get_config` 返回 `__B14.backend`（**可被测试改写**）而不是恒定快照
 *      ——真实场景里外部改动到达本页之前就已落在后端，桩必须保真，否则会制造假失败。
 *
 * ── 判据 ──────────────────────────────────────────────────────
 *   A 组：经真实回调，本地未落盘改动**存活**（含 `await` 之后）、外部改动**同时生效**、
 *         `configBase` **不含**本地改动（保证下次 save 仍会提交它）
 *   B 组：**可证伪** —— 显式 `keepLocalEdits: false`（等价于修复前的整份替换）时，
 *         本地改动**必须丢失**。若 B 组也「通过」，说明判据恒真、证明不了任何事。
 *   C 组：**调用点**必须传 `keepLocalEdits: true`（读真实回调的
 *         `Function.prototype.toString()`，不是读注释）
 *   D 组：**深比较实现** —— 本页大量**就地改嵌套对象**
 *         （`config.device_names[id] = name`，引用不变），若误写成引用相等
 *         （`a === b`）则必然漏判，该断言会转红。
 *   E 组：**三方冲突**（基线/本地/快照互不相同）⇒ 采纳后端并 `console.warn` 点名
 *         ——不与会对配置做归一化的后端对峙（P3-9）
 *   F 组：快照已含本地值 ⇒ 幂等，不残留多余补丁
 *
 * ── 副作用与清理 ──────────────────────────────────────────────
 * 需要把探针 HTML 写到 `dist/` 下（脚本用相对路径引用，故必须同目录），
 * 名字固定为 `__probe_b14.html`，在 `finally` 中删除；`--inject-broken`
 * 模式会临时改写 `dist/scripts/settings.js`，同样在 `finally` 中还原。
 * 运行后用 `git status --porcelain` 确认无残留。
 */
import fs from "node:fs";
import path from "node:path";
import { execFileSync } from "node:child_process";

const ROOT = path.resolve(import.meta.dirname, "..");
const DIST = path.join(ROOT, "src-tauri", "dist");
const SETTINGS_JS = path.join(DIST, "scripts", "settings.js");
const PROBE = path.join(DIST, "__probe_b14.html");

const EDGE_CANDIDATES = [
  "C:/Program Files (x86)/Microsoft/Edge/Application/msedge.exe",
  "C:/Program Files/Microsoft/Edge/Application/msedge.exe",
];

const INJECT_BROKEN = process.argv.includes("--inject-broken");

// ── 1. 早于所有脚本执行的 Tauri 桩 ────────────────────────────────
const STUB = `<script>
(function () {
  var BASE = {
    auto_start: false, hidden_devices: [], hidden_groups: ["Battery", "Monitor"],
    device_names: {}, device_groups: {}, filter_enabled: true, filter_regex: "^x$",
    dedup_devices: true, show_unnamed_bt: false, use_system_bt: false,
    wireless_only: true, tray_devices: [], hidden_audio_devices: [],
    log_level: "standard", log_retention: "7days",
    shutdown_volume_enabled: false, shutdown_volume_devices: {},
    mute_lock: false, volume_fine_adjust: false, force_mute_devices: [],
    enable_spatial_sound: false, check_updates: true, include_prerelease: false,
    simplify_device_names: true, shortcut_devices: null, shortcut_volume: null,
    shortcut_volume_up: null, shortcut_volume_down: null, shortcut_volume_mute: null,
    hardware_acceleration: false, default_popup_tab: "devices", popup_size: "default",
    device_shortcuts: {}, enable_device_shortcut_cycle: false,
    shortcut_switch_notify: false, theme_mode: "follow_system",
    window_material: "default", low_battery_notify: false, low_battery_devices: [],
    low_battery_thresholds: [20, 10], low_battery_refresh_secs: 60
  };
  window.__B14 = { base: BASE, backend: JSON.parse(JSON.stringify(BASE)), handlers: {}, calls: [] };
  window.__TAURI__ = {
    core: {
      invoke: function (cmd) {
        window.__B14.calls.push(cmd);
        switch (cmd) {
          // ⚠️ 必须返回**后端当前状态**（可被测试改写），而不是恒定的初始快照：
          // 真实场景里「外部改动」在到达本页之前就已经落在后端了，所以紧随其后的
          // 重新拉取应当返回**含该改动**的状态。桩若不保真，会制造假失败。
          case "get_config":
            return Promise.resolve(JSON.parse(JSON.stringify(window.__B14.backend)));
          case "get_devices": case "get_audio_devices": case "get_audio_sessions":
            return Promise.resolve([]);
          case "get_update_status": return Promise.resolve({ state: "idle" });
          default: return Promise.resolve(null);
        }
      }
    },
    event: {
      listen: function (name, handler) {
        window.__B14.handlers[name] = handler;
        return Promise.resolve(function () {});
      }
    }
  };
})();
</script>`;

// ── 2. 页面末尾的断言脚本 ────────────────────────────────────────
const ASSERTIONS = `<pre id="__b14_out"></pre>
<script>
(async function () {
  var R = [];
  function ok(name, cond, detail) {
    R.push((cond ? "PASS" : "FAIL") + " | " + name + (detail ? " | " + detail : ""));
  }
  function sleep(ms) { return new Promise(function (r) { setTimeout(r, ms); }); }
  function clone(o) { return JSON.parse(JSON.stringify(o)); }
  // 重置为「刚从后端拿到」的状态：必须显式 keepLocalEdits:false，
  // 否则上一轮的本地改动会被重放进来，污染下一轮。
  function reset() {
    window.__B14.backend = clone(window.__B14.base);
    acceptConfig(clone(window.__B14.base), { keepLocalEdits: false });
  }

  try {
    for (var i = 0; i < 100; i++) {
      if (typeof settingsNavReady !== "undefined" && settingsNavReady === true) break;
      await sleep(30);
    }
    ok("init 完成（settingsNavReady=true）",
       typeof settingsNavReady !== "undefined" && settingsNavReady === true);

    var handler = window.__B14.handlers["config-changed"];
    ok("真实 config-changed 回调已注册", typeof handler === "function");

    // ── C 组：调用点（读真实回调源码，不读注释）──
    var src = typeof handler === "function" ? handler.toString() : "";
    ok("C1 真实回调源码含 keepLocalEdits: true", src.indexOf("keepLocalEdits: true") >= 0,
       "len=" + src.length);

    // ── D 组：深比较实现 ──
    reset();
    ok("D0 装载后无本地改动 ⇒ localPendingPatch() 为 null", localPendingPatch() === null);
    config.device_names["dev-1"] = "就地改";   // 引用不变，内容已变
    var p = localPendingPatch();
    ok("D1 就地改嵌套对象被捕获（引用相等实现会漏）",
       !!p && !!p.device_names && p.device_names["dev-1"] === "就地改");

    // ── A 组：经真实回调，本地未落盘改动必须存活 ──
    async function runCase(useRealHandler) {
      reset();
      config.auto_start = true;                  // 本地：标量
      config.device_names["dev-1"] = "本地改名";  // 本地：嵌套字段
      var payload = clone(window.__B14.base);    // 外部：只动 hidden_devices
      payload.hidden_devices = ["dev-2"];
      // 外部改动**先落到后端**（真实时序就是这样：后端 emit 时自己已经是新状态）
      window.__B14.backend = clone(payload);
      var midAuto = null, midExternal = null, midName = null;
      if (useRealHandler) {
        // 先不 await：async 函数体在首个 await 前同步执行，可借此区分
        // 「acceptConfig 本身错」与「后续 await 期间被别的渲染路径覆盖」。
        var pending = handler({ payload: payload });
        midAuto = config.auto_start;
        midExternal = JSON.stringify(config.hidden_devices);
        midName = config.device_names["dev-1"];
        try { await pending; } catch (e) { /* 桩不全时渲染可能抛错 */ }
        if (midName !== "本地改名") {
          R.push("INFO | 同步阶段本地改动就已丢失 ⇒ acceptConfig 逻辑本身有问题");
        }
      } else {
        // ← 等价于修复前的调用形态：**整份替换**（必须显式传 false，
        //   因为现在的默认值是「保留本地改动」）
        acceptConfig(payload, { keepLocalEdits: false });
      }
      var patch = localPendingPatch();
      return {
        autoStart: config.auto_start,
        localName: config.device_names["dev-1"],
        external: JSON.stringify(config.hidden_devices),
        baseAuto: configBase.auto_start,
        baseName: (configBase.device_names || {})["dev-1"],
        patchKeys: patch ? Object.keys(patch).sort().join(",") : "(null)"
      };
    }

    var fixed = await runCase(true);
    ok("A1 本地改动·标量存活（含 await 之后）", fixed.autoStart === true,
       "auto_start=" + fixed.autoStart);
    ok("A2 本地改动·嵌套字段存活（含 await 之后）", fixed.localName === "本地改名",
       "device_names.dev-1=" + fixed.localName);
    ok("A3 外部改动同时生效（hidden_devices）", fixed.external === '["dev-2"]', fixed.external);
    ok("A4 configBase 不含本地改动（下次 save 仍会提交它）",
       fixed.baseAuto === false && fixed.baseName === undefined,
       "base.auto_start=" + fixed.baseAuto + ", base.dev-1=" + fixed.baseName);
    ok("A5 待落盘补丁恰为本地那两项", fixed.patchKeys === "auto_start,device_names",
       fixed.patchKeys);

    // ── E 组：三方冲突 ⇒ 采纳后端并 warn（不与会归一化的后端对峙）──
    var warns = [];
    var origWarn = console.warn;
    console.warn = function () {
      warns.push(Array.prototype.slice.call(arguments).join(" "));
    };
    reset();
    config.popup_size = "large";                 // 本地值
    var payload2 = clone(window.__B14.base);
    payload2.popup_size = "small";               // 后端也改了这个字段（模拟归一化/外部覆盖）
    payload2.hidden_devices = ["dev-9"];
    acceptConfig(payload2);
    console.warn = origWarn;
    ok("E1 三方冲突时采纳后端值", config.popup_size === "small", "popup_size=" + config.popup_size);
    ok("E2 冲突字段被 warn 点名", warns.join("").indexOf("popup_size") >= 0, warns.join(" | "));
    ok("E3 冲突不干扰其他字段的采纳", JSON.stringify(config.hidden_devices) === '["dev-9"]',
       JSON.stringify(config.hidden_devices));

    // ── F 组：快照已含本地值 ⇒ 幂等，不产生多余补丁 ──
    reset();
    config.auto_start = true;
    var payload3 = clone(window.__B14.base);
    payload3.auto_start = true;                  // 快照里已经是本地值
    acceptConfig(payload3);
    ok("F1 快照已含本地值 ⇒ 不残留待落盘补丁", localPendingPatch() === null,
       String(localPendingPatch()));

    // ── B 组：可证伪（整份替换时必须丢）──
    var old = await runCase(false);
    ok("B1 可证伪：整份替换时本地改动**丢失**",
       old.autoStart === false && old.localName === undefined,
       "auto_start=" + old.autoStart + ", device_names.dev-1=" + old.localName);
  } catch (e) {
    R.push("FAIL | 断言脚本自身抛错 | " + (e && e.message ? e.message : String(e)));
  }

  document.getElementById("__b14_out").textContent = "\\n" + R.join("\\n") + "\\n";
  document.title = R.some(function (l) { return l.indexOf("FAIL") === 0; }) ? "B14-RED" : "B14-GREEN";
})();
</script>`;

// ── 3. 生成探针 HTML ────────────────────────────────────────────
const html = fs.readFileSync(path.join(DIST, "settings.html"), "utf8");
const anchor = '<script src="scripts/common.js"></script>';
if (!html.includes(anchor)) {
  console.error(`✗ 未在 settings.html 中找到锚点：${anchor}`);
  process.exit(2);
}
const probeHtml = html
  .replace(anchor, `${STUB}\n  ${anchor}`)
  .replace("</body>", `${ASSERTIONS}\n</body>`);

// ── 4. 可证伪注入：临时改坏调用点 ───────────────────────────────
const originalSettings = fs.readFileSync(SETTINGS_JS, "utf8");
let injected = false;
const edge = EDGE_CANDIDATES.find((p) => fs.existsSync(p));
if (!edge) {
  console.error("✗ 未找到 msedge.exe");
  process.exit(2);
}

let exitCode = 0;
try {
  if (INJECT_BROKEN) {
    const broken = originalSettings.replace(
      /acceptConfig\(event\.payload, \{ keepLocalEdits: true \}\)/,
      "acceptConfig(event.payload, { keepLocalEdits: false })"
    );
    if (broken === originalSettings) {
      console.error("✗ 注入失败：未匹配到调用点（脚本与源码已漂移，请先修脚本）");
      process.exit(2);
    }
    fs.writeFileSync(SETTINGS_JS, broken);
    injected = true;
  }

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

  const m = out.match(/<pre id="__b14_out">([\s\S]*?)<\/pre>/);
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

  console.log(`── B14 验收（${INJECT_BROKEN ? "可证伪注入模式：期望 A 组转红" : "正常模式：期望全 PASS"}）──`);
  for (const l of lines) console.log("  " + l);

  const failed = lines.filter((l) => l.startsWith("FAIL"));
  const groupA = lines.filter((l) => /^(PASS|FAIL) \| A\d/.test(l));
  const aRed = groupA.some((l) => l.startsWith("FAIL"));

  if (INJECT_BROKEN) {
    // 注入模式下，A 组必须至少有一条转红，否则判据恒真
    if (groupA.length === 0) { console.error("\n✗ A 组无结果，无法判定"); exitCode = 2; }
    else if (!aRed) { console.error("\n✗ 注入后 A 组仍全绿 ⇒ 判据恒真、证明不了任何事"); exitCode = 1; }
    else console.log(`\n✓ 可证伪成立：注入后 A 组有 ${groupA.filter((l) => l.startsWith("FAIL")).length} 条转红`);
  } else {
    if (failed.length) { console.error(`\n✗ ${failed.length} 条断言失败`); exitCode = 1; }
    else console.log("\n✓ 全部断言通过");
  }
} finally {
  if (injected) {
    fs.writeFileSync(SETTINGS_JS, originalSettings);
    console.log("（已还原 settings.js 的注入）");
  }
  if (fs.existsSync(PROBE)) fs.unlinkSync(PROBE);
}
process.exit(exitCode);

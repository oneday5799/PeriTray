/**
 * CDP 驱动工具：对**真实运行的 PeriTray 进程**里的 WebView2 页面求值 / 点击。
 *
 * 用法：
 *   node tools/cdp-eval.mjs <page匹配子串> <JS 表达式>        # 求值并打印结果
 *   node tools/cdp-eval.mjs --list                            # 列出所有页面 target
 *
 * 前置：应用必须以 `--remote-debugging-port=<port>` 启动，例如
 *   WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS="--disable-gpu-sandbox --remote-debugging-port=9222" \
 *     ./target/debug/PeriTray.exe
 *
 * 为什么需要它：本仓的前端验收（B14 / L1-L2）用的是「无头 Edge + 探针 HTML」——
 * 那是**离线桩**，证明不了「真实应用里跑起来是什么样」。本工具补上这一层：
 * 连的是**真实进程**、**真实 dist**、**真实 IPC**。
 *
 * 依赖：Node 22+ 的内置全局 `WebSocket`（无需 npm 包）。
 */

const PORT = process.env.CDP_PORT || "9222";
const BASE = `http://127.0.0.1:${PORT}`;

async function listTargets() {
  const res = await fetch(`${BASE}/json`);
  return res.json();
}

/** 在指定 target 上求值。awaitPromise=true 以便直接 await 前端里的 Promise。 */
async function evaluate(wsUrl, expression) {
  const ws = new WebSocket(wsUrl);
  await new Promise((resolve, reject) => {
    ws.onopen = resolve;
    ws.onerror = (e) => reject(new Error(`WebSocket 连接失败：${e.message || e.type}`));
  });

  let nextId = 1;
  const pending = new Map();
  const events = [];
  ws.onmessage = (ev) => {
    const msg = JSON.parse(ev.data);
    if (msg.id && pending.has(msg.id)) {
      pending.get(msg.id)(msg);
      pending.delete(msg.id);
    } else if (msg.method) {
      events.push(msg);
    }
  };
  const send = (method, params = {}) =>
    new Promise((resolve) => {
      const id = nextId++;
      pending.set(id, resolve);
      ws.send(JSON.stringify({ id, method, params }));
    });

  await send("Runtime.enable");
  const r = await send("Runtime.evaluate", {
    expression,
    returnByValue: true,
    awaitPromise: true,
    userGesture: true,
  });
  ws.close();

  if (r.error) throw new Error(`CDP 错误：${JSON.stringify(r.error)}`);
  const res = r.result || {};
  if (res.exceptionDetails) {
    const d = res.exceptionDetails;
    throw new Error(
      `页面内异常：${d.exception?.description || d.text}${d.lineNumber != null ? ` @line ${d.lineNumber}` : ""}`
    );
  }
  return res.result?.value;
}

const [arg1, arg2] = process.argv.slice(2);

if (!arg1) {
  console.error("用法：node tools/cdp-eval.mjs <page匹配子串|--list> <JS 表达式>");
  process.exit(2);
}

if (arg1 === "--list") {
  const targets = await listTargets();
  for (const t of targets) {
    console.log(`${t.type}\t${t.title}\t${t.url}\n\t${t.webSocketDebuggerUrl}`);
  }
  process.exit(0);
}

const targets = await listTargets();
const hit = targets.find((t) => t.url.includes(arg1) || t.title.includes(arg1));
if (!hit) {
  console.error(`未找到匹配「${arg1}」的页面。当前有：`);
  for (const t of targets) console.error(`  ${t.title}  ${t.url}`);
  process.exit(3);
}

try {
  const value = await evaluate(hit.webSocketDebuggerUrl, arg2);
  console.log(typeof value === "string" ? value : JSON.stringify(value, null, 2));
} catch (e) {
  console.error(String(e.message || e));
  process.exit(1);
}

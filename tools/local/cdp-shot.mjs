#!/usr/bin/env node
// 临时诊断脚本：抓指定页面的 WebView 自绘位图（Page.captureScreenshot，
// 不含 DWM 叠加层），用于判定「popup 顶部条带」是页面内容还是系统绘制。
// 用法：node cdp-shot.mjs <页面标题子串> <输出png路径>
// 依赖：Node 22+ 的内置全局 `WebSocket`（与 cdp-eval.mjs 同款，无需 npm 包）。
import http from "node:http";

const [,, titleSub, outPath] = process.argv;
if (!titleSub || !outPath) { console.error("用法: node cdp-shot.mjs <标题子串> <输出.png>"); process.exit(1); }

function httpGetJson(url) {
  return new Promise((resolve, reject) => {
    http.get(url, (res) => { let d = ""; res.on("data", (c) => (d += c)); res.on("end", () => { try { resolve(JSON.parse(d)); } catch (e) { reject(e); } }); }).on("error", reject);
  });
}

const targets = await httpGetJson("http://127.0.0.1:9222/json");
const t = targets.find((x) => x.type === "page" && x.title.includes(titleSub));
if (!t) { console.error("找不到目标页面: " + titleSub); process.exit(1); }

const ws = new WebSocket(t.webSocketDebuggerUrl);
await new Promise((resolve, reject) => {
  ws.onopen = resolve;
  ws.onerror = (e) => reject(new Error(`WebSocket 连接失败：${e.message || e.type}`));
});

let nextId = 1;
const pending = new Map();
ws.onmessage = (ev) => {
  const msg = JSON.parse(ev.data);
  if (msg.id && pending.has(msg.id)) { pending.get(msg.id)(msg); pending.delete(msg.id); }
};
const send = (method, params = {}) =>
  new Promise((resolve) => { const i = nextId++; pending.set(i, resolve); ws.send(JSON.stringify({ id: i, method, params })); });

await send("Page.enable");
const shot = await send("Page.captureScreenshot", { format: "png" });
if (shot.result && shot.result.data) {
  const { writeFileSync } = await import("node:fs");
  writeFileSync(outPath, Buffer.from(shot.result.data, "base64"));
  console.log("SAVED " + outPath);
} else {
  console.error("截图失败: " + JSON.stringify(shot).substring(0, 200));
}
ws.close();
process.exit(0);

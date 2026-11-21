#!/usr/bin/env node
// 原生崩溃自取证的活体验证脚本（可证伪对照）：
// ① 负例：未装过滤器的进程触发原生 AV ⇒ 不得产生 native_crash_* 文件
//    （证明正例的文件确实来自过滤器，而非 WER 或其他机制）
// ② 正例：装了过滤器的进程触发原生 AV ⇒ 必须产生 native_crash_*.txt 与 .dmp
// 用法：node tools/local/verify-native-crash-capture.mjs
// ⚠ 两个用例都会让各自测试进程崩溃退出，属预期；脚本报「验证通过/失败」。

import { execSync, execFileSync } from "node:child_process";
import { rmSync, readdirSync, readFileSync, existsSync } from "node:fs";
import { resolve } from "node:path";

const ROOT = resolve(import.meta.dirname, "..", "..", "src-tauri");
const DIR = resolve(process.env.TEMP || "/tmp", "peritray_crash_capture_e2e");

// ── 准备：清空活体验证目录 ──
rmSync(DIR, { recursive: true, force: true });

// ── ① 负例：进程未装过滤器 ──
let negativeCrashed = false;
try {
  execSync("cargo test native_av_without -- --ignored --nocapture", {
    cwd: ROOT,
    stdio: "pipe",
    encoding: "utf8",
  });
} catch {
  // 测试触发 AV ⇒ cargo 必然非零退出；不抛异常反而说明 AV 没触发成功
  negativeCrashed = true;
}
const afterNegative = existsSync(DIR) ? readdirSync(DIR).filter((n) => n.startsWith("native_crash_")) : [];
if (afterNegative.length !== 0) {
  console.error(`✗ 负例失败：未装过滤器却产生了取证文件：${afterNegative.join(", ")}`);
  process.exit(1);
}
if (!negativeCrashed) {
  console.error("✗ 负例异常：AV 没有让测试进程崩溃——注入未生效，本次验证无效");
  process.exit(1);
}
console.log("① 负例通过：未装过滤器 ⇒ 无取证文件（进程确实崩了）");

// ── ② 正例：进程装了过滤器 ──
try {
  execSync("cargo test native_capture_writes -- --ignored --nocapture", {
    cwd: ROOT,
    stdio: "pipe",
    encoding: "utf8",
  });
} catch {
  // 预期：进程随 AV 终止
}
const files = existsSync(DIR) ? readdirSync(DIR).filter((n) => n.startsWith("native_crash_")) : [];
const txt = files.find((n) => n.endsWith(".txt"));
const dmp = files.find((n) => n.endsWith(".dmp"));
if (!txt || !dmp) {
  console.error(`✗ 正例失败：AV 后未落盘取证文件（目录内容：${files.join(", ") || "空"}）`);
  process.exit(1);
}
console.log(`② 正例通过：已落盘 ${txt} + ${dmp}`);
console.log("──────── 取证 txt 内容 ────────");
console.log(readFileSync(resolve(DIR, txt), "utf8").trimEnd());
console.log("──────── 验证通过 ────────");
// explorer 返回码不可靠（经常 1），只做打开目录的尽力而为
import { spawn } from "node:child_process";
spawn("explorer", [DIR], { detached: true, stdio: "ignore" }).unref();

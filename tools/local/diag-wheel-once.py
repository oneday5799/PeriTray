# -*- coding: utf-8 -*-
"""单次滚轮注入诊断：到底进了几条消息、方向是 up 还是 down、窗口有几个实例。"""
import ctypes
import os
import re
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def newest_log():
    d = os.path.join(WD, "logs")
    fs = [os.path.join(d, f) for f in os.listdir(d) if f.endswith(".log")]
    fs.sort(key=os.path.getmtime, reverse=True)
    return fs[0] if fs else None


subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
time.sleep(2)
for f in os.listdir(os.path.join(WD, "logs")):
    if f.endswith(".log"):
        os.remove(os.path.join(WD, "logs", f))
env = dict(os.environ)
env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--disable-gpu-sandbox --remote-debugging-port=9222"
subprocess.Popen([os.path.join(WD, "PeriTray.exe")], cwd=WD, env=env)
time.sleep(14)

log = newest_log()
txt = io_read(log)
m = re.findall(r"\[widget\] 音量命中行\(屏幕\): (\[.*?\])", txt)
if not m:
    print("!! 没等到命中行日志")
    sys.exit(2)
print("命中行:", m[-1][:150])

user32 = ctypes.windll.user32
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
mm = re.search(r"#0 \((-?\d+),(-?\d+),(-?\d+),(-?\d+)\)", m[-1])
l, t, r, b = (int(x) for x in mm.groups())
cx, cy = (l + r) // 2, (t + b) // 2
print("注入点:", (cx, cy))
user32.SetCursorPos(cx, cy)
time.sleep(0.6)

before = len(re.findall(r"滚轮调音量|滚轮未命中", txt))
# 只注入 **一次** +120
user32.mouse_event(0x0800, 0, 0, 120, 0)
time.sleep(2.5)
txt = io_read(log)
after = len(re.findall(r"滚轮调音量|滚轮未命中", txt))
print("实例数(tasklist PeriTray.exe 行数):",
      max(0, subprocess.run(["tasklist"], capture_output=True, text=True,
                            errors="replace").stdout.lower().count("peritray.exe") - 1))
print("本次新增消息数:", after - before)
for line in txt.splitlines():
    if "滚轮" in line:
        print("  ", line.split("] ", 1)[-1])

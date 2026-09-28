"""测「滚轮 → 快照更新」的延迟（乐观更新的直接证据）。

判据：注入一格滚轮后，`滚轮音量 id=` 这条日志（写在**写完音量、且乐观更新快照之后**）
出现的时刻。它同时代表「音量已写」与「快照已更新 ⇒ 紧接着 post_refresh 重绘」。
"""
import ctypes
import os
import re
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def newest_log():
    d = os.path.join(WD, "logs")
    fs = sorted(
        [os.path.join(d, f) for f in os.listdir(d) if f.endswith(".log")],
        key=os.path.getmtime,
        reverse=True,
    )
    return fs[0] if fs else None


subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
time.sleep(2)
env = dict(os.environ)
env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
    "--disable-gpu-sandbox --remote-debugging-port=9222"
subprocess.Popen([os.path.join(WD, "PeriTray.exe")], cwd=WD, env=env)
time.sleep(14)

log = newest_log()
txt = io_read(log)
m = re.findall(r"\[widget\] 滚轮触发区\(屏幕,同 tooltip\): (\[.*?\])", txt)
if not m:
    print("!! 无触发区日志")
    sys.exit(2)
mm = re.search(r"#0 \((-?\d+),(-?\d+),(-?\d+),(-?\d+)\)", m[-1])
l, t, r, b = (int(x) for x in mm.groups())
u = ctypes.windll.user32
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
u.SetCursorPos(l + max(2, (r - l) // 4), t + max(2, (b - t) // 4))
time.sleep(0.6)

n0 = len(re.findall(r"滚轮音量 id=", txt))
t0 = time.time()
u.mouse_event(0x0800, 0, 0, 120, 0)
wrote = None
while time.time() - t0 < 15:
    if len(re.findall(r"滚轮音量 id=", io_read(log))) > n0:
        wrote = time.time() - t0
        break
    time.sleep(0.03)
print(f"写入+快照更新耗时: {wrote * 1000:.0f}ms" if wrote else "!! 15s 内没出现写入日志")

# ⚠️ 必须收尾杀掉应用：否则它会一直持有本脚本的输出管道，
#    外层命令的清理步骤会一直等它退出（表现为工具报 ChildProcess.kill）。
subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)

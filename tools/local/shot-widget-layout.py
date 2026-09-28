"""任务栏 widget「图标放大 + 右上电量 / 右下音量」布局的真机效果截图。

⭐ 与 `shot-widget-m3.py` 的差别：这里**同时 pin 两台**，刻意覆盖两种取值形态：
  · 一台**有音频端点**的设备  ⇒ 右下角应显示音量百分比（或「静音」）
  · 一台**无音频端点**的设备  ⇒ 右下角应显示 `N/A`（本次用户要求的降级形态）
  ⇒ 一张图就能同时验证「右上/右下分列」与「缺失显示 N/A」两件事。

产出：`widget-layout-full.png`（任务栏整条）、`widget-layout-crop.png`（4× 放大裁剪）。
"""

import ctypes
import ctypes.wintypes as w
import glob
import os
import re
import shutil
import struct
import subprocess
import time
import zlib

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.layoutbak")
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"
OUT = r"D:\Code\PeriTray\tools\local\_out"

CAPTUREBLT, SRCCOPY = 0x40000000, 0x00CC0020
u, g = ctypes.windll.user32, ctypes.windll.gdi32
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    u.SetProcessDPIAware()


class BIH(ctypes.Structure):
    _fields_ = [
        ("biSize", w.DWORD), ("biWidth", w.LONG), ("biHeight", w.LONG),
        ("biPlanes", w.WORD), ("biBitCount", w.WORD), ("biCompression", w.DWORD),
        ("biSizeImage", w.DWORD), ("biXPelsPerMeter", w.LONG),
        ("biYPelsPerMeter", w.LONG), ("biClrUsed", w.DWORD), ("biClrImportant", w.DWORD),
    ]


def grab(y0, height):
    cx = u.GetSystemMetrics(0)
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    hbm = g.CreateCompatibleBitmap(hdc, cx, height)
    g.SelectObject(mem, hbm)
    # ⛔ 抓分层窗必须加 CAPTUREBLT，否则 widget 内容抓不到
    g.BitBlt(mem, 0, 0, cx, height, hdc, 0, y0, SRCCOPY | CAPTUREBLT)
    bi = BIH()
    bi.biSize, bi.biWidth, bi.biHeight = ctypes.sizeof(BIH), cx, -height
    bi.biPlanes, bi.biBitCount = 1, 32
    buf = ctypes.create_string_buffer(cx * height * 4)
    g.GetDIBits(mem, hbm, 0, height, buf, ctypes.byref(bi), 0)
    g.DeleteObject(hbm)
    g.DeleteDC(mem)
    u.ReleaseDC(0, hdc)
    raw = buf.raw
    return cx, lambda x, y: (
        raw[(y * cx + x) * 4 + 2], raw[(y * cx + x) * 4 + 1], raw[(y * cx + x) * 4]
    )


def save_png(path, w_, h_, getpx, scale=1):
    raw = b""
    for y in range(h_):
        raw += (b"\x00" + b"".join(bytes(getpx(x, y)) * scale for x in range(w_))) * scale

    def chunk(t, d):
        c = t + d
        return struct.pack(">I", len(d)) + c + struct.pack(">I", zlib.crc32(c) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w_ * scale, h_ * scale, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(raw, 6))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def log_text():
    logs = sorted(glob.glob(os.path.join(WD, "logs", "*.log")), key=os.path.getmtime)
    return open(logs[-1], "r", encoding="utf-8", errors="replace").read() if logs else ""


def widget_hwnd():
    m = re.findall(r"mount report: hwnd=(0x[0-9a-f]+)", log_text())
    if not m:
        return 0
    h = int(m[-1], 16)
    return h if u.IsWindow(h) else 0


def cdp(expr):
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
time.sleep(1)
shutil.copy2(CFG, CFG_BAK)
cfg = re.sub(r"pinned_taskbar_devices\s*=\s*\[[^\]]*\]\n?", "",
             open(CFG, encoding="utf-8").read())
open(CFG, "w", encoding="utf-8").write(cfg)

env = dict(os.environ)
env["PM_DEV_OPEN_SETTINGS"] = "1"
env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--disable-gpu-sandbox --remote-debugging-port=9222"
p = subprocess.Popen([EXE], cwd=WD, env=env)
time.sleep(9)

# 挑两台：一台有音频端点、一台没有（键鼠）—— 覆盖「有值」与「N/A」两种右下角。
# ⛔ 必须用 `get_taskbar_devices`（返回完整 `PhysicalDevice`）来挑：
#    `get_selectable_devices` 只返回 {key,name,fallback,sources,pinned}，
#    **没有** `audio_device_id` / `battery` ⇒ 用它做过滤永远匹配不到（实测 pinned=[]）。
PIN = ("(async () => {"
       "  const inv = window.__TAURI__.core.invoke;"
       "  const devs = await inv('get_taskbar_devices');"
       "  const audio = (devs || []).find(d => d.audio_device_id && d.volume != null);"
       "  const plain = (devs || []).find(d => !d.audio_device_id && d.battery != null);"
       "  let pick = [audio, plain].filter(Boolean);"
       "  if (pick.length < 2) { pick = (devs || []).slice(0, 2); }"
       "  for (const d of pick) {"
       "    await inv('toggle_pinned_taskbar_device',"
       "      { key: d.key, fallback: null, alias: null });"
       "  }"
       "  return JSON.stringify({pick: pick.map(d => ({n: d.name, a: !!d.audio_device_id,"
       "    b: d.battery, v: d.volume, m: d.is_muted})), total: (devs || []).length}); })()")
print("pinned:", cdp(PIN))
time.sleep(5)

tray = w.RECT()
u.GetWindowRect(u.FindWindowW("Shell_TrayWnd", None), ctypes.byref(tray))
H = tray.bottom - tray.top
print(f"taskbar rect: {tray.left},{tray.top}..{tray.right},{tray.bottom}  H={H}")

cx, px = grab(tray.top, H)
save_png(os.path.join(OUT, "widget-layout-full.png"), cx, H, px)

hwnd = widget_hwnd()
if hwnd:
    r = w.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    cw = min(cx, r.right) - max(0, r.left)
    ch = min(H, r.bottom - tray.top) - max(0, r.top - tray.top)
    print(f"widget rect {r.left},{r.top}..{r.right},{r.bottom}  size {r.right-r.left}x{r.bottom-r.top}")
    if cw > 0 and ch > 0:
        save_png(os.path.join(OUT, "widget-layout-crop.png"), cw, ch,
                 lambda x, y: px(max(0, r.left) + x, max(0, r.top - tray.top) + y), scale=4)
    # 垂直居中的判据：widget 上下留白应大致相等
    top_gap = r.top - tray.top
    bot_gap = tray.bottom - r.bottom
    print(f"垂直留白: 上={top_gap} 下={bot_gap}  (居中应大致相等)")
else:
    print("找不到 widget 窗口")

for line in log_text().splitlines():
    if "[widget]" in line and ("定位:" in line or "mount report" in line):
        print("LOG:", line.strip())

p.terminate()
time.sleep(1)
subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
shutil.copy2(CFG_BAK, CFG)
print("config 已还原")

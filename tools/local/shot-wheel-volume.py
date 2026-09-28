"""任务栏音量滚轮：把某端点音量设成 12.5%，截**任务栏那一条**，肉眼核对显示格式。

⚠️ 只截任务栏那一条（不截整屏）：减少打扰，也避免泄露无关窗口内容。
产出：`wheel-volume-display.png`。
"""
import ctypes
import ctypes.wintypes as w
import glob
import json
import os
import re
import struct
import subprocess
import sys
import time
import zlib

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"
OUT = r"D:\Code\PeriTray\tools\local\wheel-volume-display.png"

CAPTUREBLT, SRCCOPY = 0x40000000, 0x00CC0020
u, g = ctypes.windll.user32, ctypes.windll.gdi32
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    u.SetProcessDPIAware()
sys.stdout.reconfigure(encoding="utf-8", errors="replace")


class BIH(ctypes.Structure):
    _fields_ = [("biSize", w.DWORD), ("biWidth", w.LONG), ("biHeight", w.LONG),
                ("biPlanes", w.WORD), ("biBitCount", w.WORD), ("biCompression", w.DWORD),
                ("biSizeImage", w.DWORD), ("biXPelsPerMeter", w.LONG),
                ("biYPelsPerMeter", w.LONG), ("biClrUsed", w.DWORD), ("biClrImportant", w.DWORD)]


def grab(y0, height, x0=0, cw=None):
    cx = cw or u.GetSystemMetrics(0)
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    hbm = g.CreateCompatibleBitmap(hdc, cx, height)
    g.SelectObject(mem, hbm)
    g.BitBlt(mem, 0, 0, cx, height, hdc, x0, y0, SRCCOPY | CAPTUREBLT)
    bi = BIH()
    bi.biSize = ctypes.sizeof(BIH)
    bi.biWidth = cx
    bi.biHeight = -height
    bi.biPlanes = 1
    bi.biBitCount = 32
    bi.biCompression = 0
    buf = ctypes.create_string_buffer(cx * height * 4)
    g.GetDIBits(mem, hbm, 0, height, buf, ctypes.byref(bi), 0)
    g.DeleteObject(hbm)
    g.DeleteDC(mem)
    u.ReleaseDC(0, hdc)
    return cx, height, buf.raw


def write_png(path, w_, h_, bgra):
    rows = bytearray()
    for y in range(h_):
        rows.append(0)
        row = bgra[y * w_ * 4:(y + 1) * w_ * 4]
        for x in range(w_):
            b, g_, r, a = row[x * 4:x * 4 + 4]
            rows += bytes((r, g_, b))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w_, h_, 8, 6, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(rows), 6))
    png += chunk(b"IEND", b"")
    open(path, "wb").write(png)


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def newest_log():
    d = os.path.join(WD, "logs")
    fs = sorted([os.path.join(d, f) for f in os.listdir(d) if f.endswith(".log")],
                key=os.path.getmtime, reverse=True)
    return fs[0] if fs else None


subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
time.sleep(2)
env = dict(os.environ)
env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
    "--disable-gpu-sandbox --remote-debugging-port=9222"
subprocess.Popen([os.path.join(WD, "PeriTray.exe")], cwd=WD, env=env)
time.sleep(15)

# 把第一个有音频端点的设备音量设成 12.5%
r = subprocess.run([NODE, CDP, "popup.html",
                    "(async()=>{const inv=window.__TAURI__.core.invoke;"
                    "const d=await inv('get_audio_devices');"
                    "const t=d.find(x=>x.volume!=null);"
                    "if(!t)return JSON.stringify({err:'no device'});"
                    "await inv('set_device_volume',{deviceId:t.id,volume:0.125});"
                    "return JSON.stringify({name:t.name,volume:t.volume});})()"],
                   capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=60)
print("设音量:", r.stdout.strip() or r.stderr.strip()[:200])
time.sleep(2.5)

txt = io_read(newest_log())
m = re.findall(r"\[widget\] 滚轮触发区\(屏幕,同 tooltip\): (\[.*?\])", txt)
if not m:
    print("!! 无触发区日志")
else:
    mm = re.search(r"#(\d+) \((-?\d+),(-?\d+),(-?\d+),(-?\d+)\)", m[-1])
    idx, l, t, rr, b = (int(x) for x in mm.groups())
    # ⭐ 只截 **widget 那一小块**（含前后各 30px 余量）：整条 1920px 的截图
    #   会被图像工具的体积上限拒掉，而这块才是要看的内容。
    pad_x, pad_y = 30, 22
    y0 = max(0, t - pad_y)
    h_ = min(u.GetSystemMetrics(1) - y0, (b - t) + pad_y * 2)
    x0 = max(0, min(l, 1200) - pad_x)
    cw = (rr + pad_x) - x0
    w_, h2, bgra = grab(y0, h_, x0, cw)
    write_png(OUT, w_, h2, bgra)
    print(f"已截 #{idx} 所在区域 → {OUT}（y={y0}, h={h2}）")
    print("触发区:", m[-1][:120])

subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)

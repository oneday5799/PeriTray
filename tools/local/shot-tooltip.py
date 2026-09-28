"""截取 tooltip 及其周边（含阴影与任务栏）存 PNG，供人工与 FluentFlyout 对比。

用法：与 app 同时启动；轮询到可见即抓。
⚠️ 抓分层/合成内容必须带 CAPTUREBLT（否则只拿到桌面背景）。
"""

import ctypes
import ctypes.wintypes as wt
import glob
import io
import os
import re
import struct
import sys
import time
import zlib

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

TTM_GETTOOLCOUNT = 1037
SMTO_ABORTIFHUNG = 0x0002
SRCCOPY, CAPTUREBLT, DIB_RGB_COLORS = 0x00CC0020, 0x40000000, 0
LOG_DIR = "D:/Code/PeriTray/src-tauri/target/debug/logs"
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)),
                   "shot-tooltip-actual.png")


class RECT(ctypes.Structure):
    _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                ("r", ctypes.c_long), ("b", ctypes.c_long)]


class BIH(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int),
                ("biHeight", ctypes.c_int), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biX", ctypes.c_long),
                ("biY", ctypes.c_long), ("biClrUsed", ctypes.c_uint32),
                ("biClrImportant", ctypes.c_uint32)]


class BI(ctypes.Structure):
    _fields_ = [("h", BIH), ("colors", ctypes.c_uint32 * 3)]


u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
u.IsWindow.restype, u.IsWindow.argtypes = wt.BOOL, [wt.HWND]
u.IsWindowVisible.restype, u.IsWindowVisible.argtypes = wt.BOOL, [wt.HWND]
u.SendMessageTimeoutW.restype = ctypes.c_uint
u.SendMessageTimeoutW.argtypes = [wt.HWND, ctypes.c_uint, ctypes.c_size_t,
                                  ctypes.c_ssize_t, ctypes.c_uint, ctypes.c_uint,
                                  ctypes.POINTER(ctypes.c_size_t)]
u.GetDC.restype, u.GetDC.argtypes = wt.HDC, [wt.HWND]
g.CreateCompatibleDC.restype, g.CreateCompatibleDC.argtypes = wt.HDC, [wt.HDC]
g.CreateCompatibleBitmap.restype = wt.HBITMAP
g.CreateCompatibleBitmap.argtypes = [wt.HDC, ctypes.c_int, ctypes.c_int]
g.SelectObject.argtypes = [wt.HDC, wt.HGDIOBJ]
g.BitBlt.restype = ctypes.c_int
g.BitBlt.argtypes = [wt.HDC] + [ctypes.c_int] * 4 + [wt.HDC] + [ctypes.c_int] * 2 + [ctypes.c_uint]
g.GetDIBits.argtypes = [wt.HDC, wt.HBITMAP, ctypes.c_uint, ctypes.c_uint,
                        ctypes.c_void_p, ctypes.POINTER(BI), ctypes.c_uint]


def send(h, m):
    r = ctypes.c_size_t()
    return u.SendMessageTimeoutW(h, m, 0, 0, SMTO_ABORTIFHUNG, 2000, ctypes.byref(r)) != 0


def tip_from_log():
    logs = sorted(glob.glob(os.path.join(LOG_DIR, "*.log")), key=os.path.getmtime)
    if not logs or (time.time() - os.path.getmtime(logs[-1])) > 60:
        return None
    txt = io.open(logs[-1], encoding="utf-8", errors="replace").read()
    m = re.findall(r"tip 窗已建 hwnd=(0x[0-9a-fA-F]+)", txt)
    if not m:
        return None
    # 句柄可能被复用/重建 ⇒ 取**最后一个仍存在**的那个
    for cand in reversed(m):
        h = int(cand, 16)
        if u.IsWindow(h):
            return h
    return None
    return h if u.IsWindow(h) else None


def grab(x, y, w, h):
    sdc = u.GetDC(None)
    mdc = g.CreateCompatibleDC(sdc)
    bmp = g.CreateCompatibleBitmap(sdc, w, h)
    g.SelectObject(mdc, bmp)
    g.BitBlt(mdc, 0, 0, w, h, sdc, x, y, SRCCOPY | CAPTUREBLT)
    bi = BI()
    bi.h.biSize, bi.h.biWidth, bi.h.biHeight = ctypes.sizeof(BIH), w, -h
    bi.h.biPlanes, bi.h.biBitCount = 1, 32
    buf = (ctypes.c_ubyte * (w * h * 4))()
    g.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bi), DIB_RGB_COLORS)
    return buf, w, h


def write_png(path, buf, w, h):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
        for x in range(w):
            i = (y * w + x) * 4
            raw += bytes((buf[i + 2], buf[i + 1], buf[i]))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


def main():
    tip = None
    deadline = time.time() + 40
    while time.time() < deadline and tip is None:
        tip = tip_from_log()
        time.sleep(0.3)
    if tip is None:
        print("❌ 未从日志解析到 tip 句柄")
        return 1

    r = RECT()
    while time.time() < deadline:
        u.GetWindowRect(tip, ctypes.byref(r))
        if (r.r - r.l) > 10 and u.IsWindowVisible(tip):
            break
        time.sleep(0.15)

    # ⭐ **立即抓，不等**。自绘版没有淡入动画，原先那 0.75s 等待反而有害：
    #   开发时鼠标可能恰好停在 widget 上，hover 路径与 `PM_DEV_TOOLTIP_SHOW`
    #   门控会互相覆盖（一个 show 一个 hide）⇒ 等久了就只拍到空气。
    #   正确做法是**连续抓几帧挑一帧有内容的**，而不是赌它一直可见。
    best = None
    for _ in range(12):
        r = RECT()
        u.GetWindowRect(tip, ctypes.byref(r))
        if (r.r - r.l) <= 10 or not u.IsWindowVisible(tip):
            time.sleep(0.2)
            continue
        buf, W, H = grab(r.l - 10, r.t - 10,
                         (r.r - r.l) + 20, (r.b - r.t) + 44)
        # 判据：窗口所在矩形内必须有足够多的**非纯黑**像素（气泡本身）
        opaque = 0
        for yy in range(20, H - 30):
            for xx in range(20, W - 20):
                i = (yy * W + xx) * 4
                if buf[i] or buf[i + 1] or buf[i + 2]:
                    opaque += 1
        if opaque > 400:
            best = (buf, W, H, r, (r.l - 10, r.t - 10))
            break
        time.sleep(0.2)
    if best is None:
        print("❌ 12 次重试都没拍到气泡内容（提示被 hover 路径隐藏？）")
        return 1
    buf, W, H, r, (ox, oy) = best
    globals()["_origin"] = (ox, oy)

    # 连同**周边 30px**一起抓 ⇒ 能看到阴影、圆角、与任务栏的相对位置
    write_png(OUT, buf, W, H)
    print("✅ 已保存 %s（区域 %dx%d，抓取原点=(%d,%d)，tip rect=(%d,%d)-(%d,%d)）"
          % (OUT, W, H, ox, oy, r.l, r.t, r.r, r.b))
    return 0


if __name__ == "__main__":
    sys.exit(main())

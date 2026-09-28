"""阶段 0 决定性像素判据：强制显示 tooltip → 立刻抓屏 → 看是否真的渲染出来。

回答两问（gate）：
  Q2  文本能否渲染？（4 条设备名是否被真正画到屏幕上）
  Q3  会不会被任务栏盖住？

⭐ 关键手法：`TTM_ACTIVATE` 是**公开 API**，能让 tooltip 在**指定 uId** 上确定性显示
⇒ 不依赖鼠标（**本机无法注入鼠标**，MEMORY.md §3）也能验收。

⚠️ 为什么必须「激活后立刻抓」：tooltip 显示约 5 秒后自动隐藏
⇒ 应用启动时激活、探针稍后才截图，必然扑空（本次首版就扑空了，
   读到 `IsWindowVisible=False` 而差点误判「不显示」）。
"""

import ctypes
import ctypes.wintypes as wt
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)

# ⭐ 本机 125% 缩放：观测进程必须先声明 PER_MONITOR_AWARE_V2，
#    否则坐标整体错位（Wiki 15 §9.1 记着这个坑）。
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

TTM_ACTIVATE = 1025
TTM_GETTOOLCOUNT = 1037
SMTO_ABORTIFHUNG = 0x0002
SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000
DIB_RGB_COLORS = 0


class RECT(ctypes.Structure):
    _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                ("r", ctypes.c_long), ("b", ctypes.c_long)]


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int),
                ("biHeight", ctypes.c_int), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biXPelsPerMeter", ctypes.c_long),
                ("biYPelsPerMeter", ctypes.c_long), ("biClrUsed", ctypes.c_uint32),
                ("biClrImportant", ctypes.c_uint32)]


class BITMAPINFO(ctypes.Structure):
    _fields_ = [("bmiHeader", BITMAPINFOHEADER), ("bmiColors", ctypes.c_uint32 * 3)]


u.FindWindowW.restype = wt.HWND
u.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
u.GetClassNameW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
u.IsWindowVisible.argtypes = [wt.HWND]
u.GetDC.restype = wt.HDC
u.GetDC.argtypes = [wt.HWND]
u.SendMessageTimeoutW.restype = ctypes.c_uint
u.SendMessageTimeoutW.argtypes = [wt.HWND, ctypes.c_uint, ctypes.c_size_t,
                                  ctypes.c_ssize_t, ctypes.c_uint, ctypes.c_uint,
                                  ctypes.POINTER(ctypes.c_size_t)]
g.CreateCompatibleDC.restype = wt.HDC
g.CreateCompatibleDC.argtypes = [wt.HDC]
g.CreateCompatibleBitmap.restype = wt.HBITMAP
g.CreateCompatibleBitmap.argtypes = [wt.HDC, ctypes.c_int, ctypes.c_int]
g.SelectObject.argtypes = [wt.HDC, wt.HGDIOBJ]
g.BitBlt.restype = ctypes.c_int
g.BitBlt.argtypes = [wt.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                     wt.HDC, ctypes.c_int, ctypes.c_int, ctypes.c_uint]
g.GetDIBits.argtypes = [wt.HDC, wt.HBITMAP, ctypes.c_uint, ctypes.c_uint,
                        ctypes.c_void_p, ctypes.POINTER(BITMAPINFO), ctypes.c_uint]
g.GetDIBits.restype = ctypes.c_int
g.DeleteDC.argtypes = [wt.HDC]
g.DeleteObject.argtypes = [wt.HGDIOBJ]


def send(hwnd, msg, wp=0, lp=0):
    res = ctypes.c_size_t()
    ok = u.SendMessageTimeoutW(hwnd, msg, wp, lp, SMTO_ABORTIFHUNG, 2000,
                                ctypes.byref(res))
    return (ok != 0, res.value)


def enum_tips():
    out = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, _):
        b = ctypes.create_unicode_buffer(256)
        u.GetClassNameW(h, b, 256)
        if b.value == "tooltips_class32":
            out.append(h)
        return True

    u.EnumWindows(cb, 0)
    return out


def grab(x, y, w, h):
    sdc = u.GetDC(None)
    mdc = g.CreateCompatibleDC(sdc)
    bmp = g.CreateCompatibleBitmap(sdc, w, h)
    g.SelectObject(mdc, bmp)
    # ⛔ 抓合成内容必须带 CAPTUREBLT，否则只拿到桌面背景
    g.BitBlt(mdc, 0, 0, w, h, sdc, x, y, SRCCOPY | CAPTUREBLT)
    bi = BITMAPINFO()
    bi.bmiHeader.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bi.bmiHeader.biWidth = w
    bi.bmiHeader.biHeight = -h
    bi.bmiHeader.biPlanes = 1
    bi.bmiHeader.biBitCount = 32
    buf = (ctypes.c_ubyte * (w * h * 4))()
    g.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bi), DIB_RGB_COLORS)
    g.DeleteDC(mdc)
    g.DeleteObject(bmp)
    return buf


def main():
    # 1) 找我们那个 tip 窗：注册数 > 0
    tip = None
    for h in enum_tips():
        ok, n = send(h, TTM_GETTOOLCOUNT)
        if ok and n > 0:
            tip = h
            print("[P] 命中 tip 窗 %#x，TTM_GETTOOLCOUNT=%d" % (h, n))
            break
    if not tip:
        print("❌ FAIL: 找不到注册了工具的 tip 窗")
        return 1

    # 2) 激活第 1 条并**立刻**抓屏（tooltip 约 5s 后自动隐藏）
    ok, _ = send(tip, TTM_ACTIVATE, 1, 0)
    print("[P] TTM_ACTIVATE(1) 已送达=%s" % ok)
    time.sleep(0.4)

    r = RECT()
    u.GetWindowRect(tip, ctypes.byref(r))
    vis = bool(u.IsWindowVisible(tip))
    print("[P] tip rect=(%d,%d)-(%d,%d) visible=%s"
          % (r.l, r.t, r.r, r.b, vis))
    w, h = r.r - r.l, r.b - r.t
    if w <= 0 or h <= 0:
        print("❌ FAIL: tip 矩形退化")
        return 1

    buf = grab(r.l, r.t, w, h)
    hist = {}
    for y in range(h):
        for x in range(w):
            i = (y * w + x) * 4
            key = (buf[i + 2], buf[i + 1], buf[i])  # B,G,R -> R,G,B
            hist[key] = hist.get(key, 0) + 1
    tot = w * h
    top = sorted(hist.items(), key=lambda kv: -kv[1])[:6]

    print("")
    print("=" * 66)
    print("抓到的区域 %d×%d = %d 像素，前 6 色：" % (w, h, tot))
    for c, k in top:
        print("   RGB%-16s %5d (%5.1f%%)" % (str(c), k, 100.0 * k / tot))
    print("=" * 66)

    # ── 判据 ──────────────────────────────────────────────────────
    # ⭐ 判据选「**色彩多样性**」而非「某��精确颜色」：tooltip 表面 + 边框 + 阴影 +
    #   抗锯齿文本 ⇒ 若真的渲染出来，**必然**存在多种颜色；任务栏是近乎纯色的一条
    #   ⇒ 单一颜色占 >92% 判为「没渲染」。
    #   ⛔ 不用绝对亮度差（同 Wiki 15 §9.2 记的坑：那是背景的函数）。
    distinct = len([c for c, k in hist.items() if k >= 3])
    dominant = top[0][1] / float(tot)
    print("")
    print("判据：颜色种类(≥3px)=%d，主流色占比=%.1f%%" % (distinct, 100 * dominant))
    if distinct < 8:
        print("❌ FAIL: 区域内几乎纯色 ⇒ tooltip **没有**真正渲染到屏幕上")
        print("   （tip 窗存在 ≠ 被画出来；Wiki 15 §E.4 记着这个教训）")
        return 1
    if dominant > 0.92:
        print("❌ FAIL: 主流色占比过高 ⇒ 抓到的是任务栏本身，tooltip 被盖住了")
        return 1
    print("✅ PASS: 区域内有 %d 种颜色 ⇒ tooltip 已渲染，且**未被**任务栏盖住" % distinct)
    return 0


if __name__ == "__main__":
    sys.exit(main())

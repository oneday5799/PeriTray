"""同口径实测「可见底边到任务栏上缘」的距离：主窗口 vs 任务栏 tooltip。

口径：只看**像素亮度**。任务栏是暗色（<115），窗口/气泡是亮色（>235），
     中间若有阴影羽化（115..235）单独标出 ⇒ 区分「实空白」与「阴影填充」。

⛔ 不用 `GetWindowRect`：实测主窗口是 `transparent(true)` 无边框窗，
   其窗口矩形**含 DWM 阴影边距**（窗口底 1374 / 可见底 1364，差 10px）
   ⇒ 用窗口矩形量会把阴影边距算成「间隙」。
"""
import ctypes
import ctypes.wintypes as wt
import sys

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
TASKBAR_TOP = 1380


class BIH(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int),
                ("biHeight", ctypes.c_int), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biX", ctypes.c_long),
                ("biY", ctypes.c_long), ("biClrUsed", ctypes.c_uint32),
                ("biZ", ctypes.c_uint32)]


class BI(ctypes.Structure):
    _fields_ = [("h", BIH), ("colors", ctypes.c_uint32 * 3)]


def grab(x, y, w, h):
    sdc = u.GetDC(None)
    mdc = g.CreateCompatibleDC(sdc)
    bmp = g.CreateCompatibleBitmap(sdc, w, h)
    g.SelectObject(mdc, bmp)
    g.BitBlt(mdc, 0, 0, w, h, sdc, x, y, 0x00CC0020 | 0x40000000)
    bi = BI()
    bi.h.biSize, bi.h.biWidth, bi.h.biHeight = ctypes.sizeof(BIH), w, -h
    bi.h.biPlanes, bi.h.biBitCount = 1, 32
    buf = (ctypes.c_ubyte * (w * h * 4))()
    g.GetDIBits(mdc, bmp, 0, h, buf, ctypes.byref(bi), 0)
    u.ReleaseDC(None, sdc)
    return buf, w


def classify(avg):
    if avg > 235:
        return "亮(窗口/气泡)"
    if avg > 115:
        return "阴影羽化"
    return "任务栏/暗"


def scan(name, x0, x1, rows=40):
    y0 = TASKBAR_TOP - rows
    buf, w = grab(x0, y0, x1 - x0, rows)
    at = lambda xx, yy: (buf[((yy * w + xx) * 4) + 2],
                         buf[((yy * w + xx) * 4) + 1],
                         buf[((yy * w + xx) * 4)])
    print("\n=== %s（x %d..%d）===" % (name, x0, x1))
    bottom = None
    for yy in range(rows - 1, -1, -1):
        row = [sum(at(x, yy)) / 3 for x in range(0, w, 3)]
        avg = sum(row) / len(row)
        if bottom is None and classify(avg) != "任务栏/暗":
            bottom = y0 + yy
        if y0 + yy >= bottom - 4 if bottom else False:
            print("  y=%d avg=%5.1f  %s" % (y0 + yy, avg, classify(avg)))
    if bottom is None:
        print("  未找到亮色内容")
        return
    print("  >>> 可见底边 y=%d ; 任务栏上缘 %d ; **可见间距 = %d px**"
          % (bottom, TASKBAR_TOP, TASKBAR_TOP - bottom))
    gap_region = []
    for yy in range(bottom + 1, TASKBAR_TOP - y0):
        row = [sum(at(x, yy)) / 3 for x in range(0, w, 3)]
        gap_region.append(classify(sum(row) / len(row)))
    if gap_region:
        n_shadow = gap_region.count("阴影羽化")
        print("  >>> 间隙内容：%s" % ("纯空白" if n_shadow == 0
                                     else "含 %d 行阴影羽化" % n_shadow))


if __name__ == "__main__":
    scan(sys.argv[1], int(sys.argv[2]), int(sys.argv[3]))

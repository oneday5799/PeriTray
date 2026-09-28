"""量「可见底边到任务栏上缘」的间距 —— 同一口径下比较主窗口与任务栏 tooltip。

⛔ 为什么不看 `GetWindowRect`：主窗口是 `transparent(true)` 的无边框窗，
   其窗口矩形**可能含 DWM 阴影边距**（实测窗口高 660px，而 `popup_h` 只请求 520
   ⇒ 多出的 8 逻辑 px 正好把 `POPUP_TASKBAR_GAP=13` 啃到 ~5）。
   ⇒ 用户看到的「视觉间距」必须从**像素**量。

用法：measure-gap.py <x0> <x1>   （x 为屏幕物理坐标，需覆盖两者的水平范围）
"""
import struct
import sys
import zlib

sys.stdout.reconfigure(encoding="utf-8")
u = None


def grab(x, y, w, h):
    import ctypes
    import ctypes.wintypes as wt
    global u
    if u is None:
        u = ctypes.WinDLL("user32", use_last_error=True)
        u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    g = ctypes.WinDLL("gdi32", use_last_error=True)
    class BIH(ctypes.Structure):
        _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int),
                    ("biHeight", ctypes.c_int), ("biPlanes", ctypes.c_uint16),
                    ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                    ("biSizeImage", ctypes.c_uint32), ("biX", ctypes.c_long),
                    ("biY", ctypes.c_long), ("biClrUsed", ctypes.c_uint32),
                    ("biBi", ctypes.c_uint32)]
    class BI(ctypes.Structure):
        _fields_ = [("h", BIH), ("colors", ctypes.c_uint32 * 3)]
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
    return buf, w, h


def main():
    x0, x1 = int(sys.argv[1]), int(sys.argv[2])
    taskbar_top = 1380
    # 从任务栏上缘往上扫 120px，找「连续属于窗口」的最高行（即可见底边）
    h = 120
    buf, w, _ = grab(x0, taskbar_top - h, x1 - x0, h)
    at = lambda xx, yy: (buf[((yy * w + xx) * 4) + 2],
                         buf[((yy * w + xx) * 4) + 1],
                         buf[((yy * w + xx) * 4)])
    # 任务栏本身是暗色（~120-135），窗口区是亮色（>200）⇒ 用亮度分界
    def bright(xx, yy):
        return sum(at(xx, yy)) / 3 > 190
    cols = [xx for xx in range(0, w, 4) if bright(xx, h - 1)]
    if not cols:
        print("该水平范围内任务栏上方没有亮色内容（窗口不在此处）")
        return
    lo, hi = min(cols), max(cols)
    for yy in range(h - 1, -1, -1):
        if bright(lo + 2, yy) and bright(hi - 2, yy):
            bottom = taskbar_top - h + yy
            break
    else:
        print("找不到可见底边")
        return
    print("可见底边 y=%d ; 任务栏上缘 %d ; **视觉间距 = %d px**"
          % (bottom, taskbar_top, taskbar_top - bottom))
    print("水平范围 x=%d..%d" % (x0 + lo, x0 + hi))


if __name__ == "__main__":
    main()

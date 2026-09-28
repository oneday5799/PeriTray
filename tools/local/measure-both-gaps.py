"""直接测出「主窗口」与「tooltip」到任务栏的**实际物理间距**，并反推各自的换算系数。

同时读取应用内部用的 `work_h`（逻辑），与实测物理间距一起反推：
  实测间距 = POPUP_TASKBAR_GAP × sf        （主窗口，Tauri Logical 换算）
  实测间距 = TIP_TASKBAR_GAP_DIP × k      （tooltip，content_px 换算）
⇒ 解出 sf 与 k，就能判断两条路径**实际**用的是哪套基准。
"""
import ctypes
import ctypes.wintypes as wt
import sys

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
TASKBAR_TOP = None


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


def taskbar_top():
    class RECT(ctypes.Structure):
        _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                    ("r", ctypes.c_long), ("b", ctypes.c_long)]
    u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
    u.FindWindowW.restype = wt.HWND
    u.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
    h = u.FindWindowW("Shell_TrayWnd", None)
    rc = RECT()
    u.GetWindowRect(h, ctypes.byref(rc))
    return rc.t, rc.b - rc.t


def visible_bottom(x0, x1, top, rows=60, ratio=0.6):
    """从任务栏上缘往上扫，找该水平范围内**最靠下**的「非任务栏」行。

    ⛔⛔ **判据必须是相对的**，不能是「亮度 > 200」这种绝对阈值：
    实测踩了两次假值 ——
      ① 绝对阈值 200：任务栏自身亮度就有 ~200~217（本机浅色任务栏）⇒
         把**任务栏**当窗口，报出「间距 1px」；
      ② ratio=0.3 + 绝对阈值：tooltip 上方那片同亮度壁纸也被当窗口。
    ⇒ 先取「任务栏那一行」的平均亮度当基准，只找**明显亮于它**的行。
    """
    buf, w = grab(x0, top - rows, x1 - x0, rows)
    at = lambda xx, yy: sum(buf[((yy * w + xx) * 4) + k] for k in range(3)) / 3
    rowavg = lambda yy: sum(at(xx, yy) for xx in range(w)) / w
    tb = rowavg(rows - 1)              # 最后一行就是任务栏
    thr = tb + 25.0                    # ⭐ 相对阈值：明显亮于任务栏
    for yy in range(rows - 1, -1, -1):
        row = [at(xx, yy) for xx in range(w)]
        bright = sum(1 for v in row if v > thr)
        if bright > w * ratio:
            return top - rows + yy
    return None


def main():
    top, tbh = taskbar_top()
    print("任务栏: y=%d..%d (高 %d)" % (top, top + tbh, tbh))
    # 先打印任务栏基准亮度，便于核对判据
    _b, _w = grab(1200, top - 4, 60, 4)
    _a = lambda xx, yy: sum(_b[((yy * _w + xx) * 4) + k] for k in range(3)) / 3
    print("任务栏基准亮度 = %.1f（判据 = 基准+25）" % (sum(_a(x, 3) for x in range(_w)) / _w))
    print()
    print("请先保证 tooltip 已显示（悬停设备上），且主窗口已打开。\n")

    # tooltip：⛔ 只扫**气泡**的横向范围（窗口 +18px 阴影边距）
    #   （扫整个 widget 宽度会命中壁纸，见 `visible_bottom` 的 ratio 说明）
    import re, os, glob
    logs = sorted(glob.glob("D:/Code/PeriTray/src-tauri/target/debug/logs/*.log"),
                  key=os.path.getmtime)
    txt = open(logs[-1], encoding="utf-8", errors="replace").read()
    m = re.findall(r"气泡=(\d+)×(\d+) 窗=\((-?\d+),(-?\d+)\)", txt)
    if not m:
        print("日志里没有 tooltip 落位记录，先悬停到设备上")
        return
    bw, bh, wx, wy = (int(v) for v in m[-1])
    tb_x0, tb_x1 = wx + 25, wx + bw - 7   # 气泡横向范围（窗口 +18 边距）
    print("tooltip 气泡 = %dx%d @ 窗口(%d,%d) ⇒ 横向扫 %d..%d"
          % (bw, bh, wx, wy, tb_x0, tb_x1))
    wt_ = visible_bottom(tb_x0, tb_x1, top)
    print("tooltip 可见底边 = %s  => 间距 = %s px"
          % (wt_, (top - wt_) if wt_ else "?"))

    # 主窗口：先找它的横向范围
    u.FindWindowW.restype = wt.HWND
    u.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
    ph = u.FindWindowW(None, "外设信息")
    if not ph:
        print("主窗口未打开，跳过")
        return
    class RECT(ctypes.Structure):
        _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                    ("r", ctypes.c_long), ("b", ctypes.c_long)]
    u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
    rc = RECT()
    u.GetWindowRect(ph, ctypes.byref(rc))
    print("主窗口 rect=(%d,%d)-(%d,%d)  窗口底距任务栏 = %d px"
          % (rc.l, rc.t, rc.r, rc.b, top - rc.b))
    # 只在中央 60% 取样，避开圆角
    cx0 = rc.l + int((rc.r - rc.l) * 0.2)
    cx1 = rc.r - int((rc.r - rc.l) * 0.2)
    pb = visible_bottom(cx0, cx1, top)
    print("主窗口可见底边 = %s  => **间距 = %s px**"
          % (pb, (top - pb) if pb else "?"))
    if pb and wt_:
        print()
        print("两者间距差 = %d px" % abs((top - pb) - (top - wt_)))


if __name__ == "__main__":
    main()

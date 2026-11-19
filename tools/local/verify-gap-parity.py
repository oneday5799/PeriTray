"""验收「主窗口与 tooltip 到任务栏的可见间距一致」。

判据（纯像素，**不用任何亮度阈值**——实测踩坑三次：
  ① 绝对阈值 >200 把**任务栏本身**当窗口（它亮度就有 200~217）；
  ② ratio=0.3 把同亮度**壁纸**当窗口；
  ③ ClientToScreen 在 ctypes 下返回错值（-1637x-65）。
⇒ 改用**相邻行亮度跳变**：窗口内容恒定亮（≈252），阴影是**每行 +1 的渐变**，
  内容→非内容处是 **−100 量级的断崖**。判据 = 找最大负跳变所在行。
"""
import ctypes
import ctypes.wintypes as wt
import re
import glob
import os
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))


class BIH(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int),
                ("biHeight", ctypes.c_int), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biX", ctypes.c_long),
                ("biY", ctypes.c_long), ("biClrUsed", ctypes.c_uint32),
                ("biZ", ctypes.c_uint32)]


class BI(ctypes.Structure):
    _fields_ = [("h", BIH), ("colors", ctypes.c_uint32 * 3)]


class RECT(ctypes.Structure):
    _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                ("r", ctypes.c_long), ("b", ctypes.c_long)]


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


def content_bottom(x0, x1, top, rows=30, plateau=None):
    """用**相邻行亮度跳变**找可见内容底边。返回 (y, 说明)。

    ⚠️ tooltip 侧不能用「最大负跳变」：它**紧邻**任务栏，而本机任务栏是**浅色**
    （亮度 ~250，与气泡的 252 几乎相同）⇒ 断崖被淹没（实测最大负跳变仅 -36）。
    ⇒ 加 `plateau` 模式：找「恒定亮区」的最后一行（连续 ≥3 行亮度方差 <1.5），
       即内容区结束处。适合「下方还有其它亮色物体」的场景。
    """
    buf, w = grab(x0, top - rows, x1 - x0, rows)
    at = lambda xx, yy: sum(buf[((yy * w + xx) * 4) + k] for k in range(3)) / 3
    avg = [sum(at(x, yy) for x in range(0, w, 2)) / (w / 2) for yy in range(rows)]
    if plateau:
        # 从下往上找连续等亮区（内容），其**最后一行**即底边
        run = 0
        for i in range(rows - 1, -1, -1):
            same = (i == rows - 1) or (abs(avg[i] - avg[i + 1]) < 1.5)
            run = run + 1 if same else 0
            if run >= 3:
                return top - rows + i, "等亮区末行（连续 %d 行）" % run
        return None, "未找到等亮区"
    best, best_i = 0.0, None
    for i in range(1, rows):
        d = avg[i] - avg[i - 1]
        if d < best:
            best, best_i = d, i
    if best_i is None or best > -40:
        return None, "未找到断崖（最大负跳变仅 %.1f）" % best
    return top - rows + best_i - 1, "断崖 %.1f" % best


def taskbar_top():
    u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
    u.FindWindowW.restype = wt.HWND
    u.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
    h = u.FindWindowW("Shell_TrayWnd", None)
    rc = RECT()
    u.GetWindowRect(h, ctypes.byref(rc))
    return rc.t, rc.b - rc.t


def popup_wait():
    u.FindWindowW.restype = wt.HWND
    u.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
    u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
    u.IsWindowVisible.argtypes = [wt.HWND]
    u.IsWindowVisible.restype = wt.BOOL
    h = u.FindWindowW(None, "外设信息")
    if not h:
        return None
    for _ in range(40):
        rc = RECT()
        u.GetWindowRect(h, ctypes.byref(rc))
        if u.IsWindowVisible(h) and rc.b <= 1380 and rc.b > 1200:
            return rc
        time.sleep(0.3)
    return None


def main():
    top, _ = taskbar_top()
    print("任务栏上缘 = %d\n" % top)
    rc = popup_wait()
    if not rc:
        print("❌ 主窗口未打开或未静止")
        return 1
    print("主窗口 rect = (%d,%d)-(%d,%d)" % (rc.l, rc.t, rc.r, rc.b))
    cx0 = rc.l + int((rc.r - rc.l) * 0.2)
    cx1 = rc.r - int((rc.r - rc.l) * 0.2)
    pb, pinfo = content_bottom(cx0, cx1, top)
    if pb is None:
        print("❌ 主窗口可见底边定位失败：%s" % pinfo)
        return 1
    print("主窗口可见底边 = %d（%s）⇒ 间距 = %d px\n" % (pb, pinfo, top - pb))

    logs = sorted(glob.glob("D:/Code/PeriTray/src-tauri/target/debug/logs/*.log"),
                  key=os.path.getmtime)
    txt = open(logs[-1], encoding="utf-8", errors="replace").read()
    m = re.findall(r"气泡=(\d+)×(\d+) 窗=\((-?\d+),(-?\d+)\)", txt)
    if not m:
        print("❌ 日志里没有 tooltip 落位（先悬停到设备上）")
        return 1
    bw, bh, wx, wy = (int(v) for v in m[-1])
    # ⚠️ tooltip 用 plateau 模式：下方紧邻**浅色**任务栏，断崖会被淹没
    tb, tinfo = content_bottom(wx + 25, wx + bw - 7, top, rows=24, plateau=True)
    if tb is None:
        print("❌ tooltip 可见底边定位失败：%s" % tinfo)
        return 1
    print("tooltip 可见底边 = %d（%s）⇒ 间距 = %d px" % (tb, tinfo, top - tb))

    gp, gt = top - pb, top - tb
    print()
    print("主窗口间距 = %d px ; tooltip 间距 = %d px ; 差 = %d px" % (gp, gt, abs(gp - gt)))
    ok = abs(gp - gt) <= 2
    print("\n%s  两者间距%s" % ("✅ 一致" if ok else "❌ 不一致",
                                "（差 ≤2px 视为圆整误差）" if ok else ""))
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())

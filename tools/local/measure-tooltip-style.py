"""量出 tooltip 的**实际渲染参数**，作为「对齐 FluentFlyout」的基线/验收。

量什么（都是 FluentFlyout `CustomToolTip.xaml` 里写明的属性）：
  · 表面色（对应 ToolTipBackground）
  · 文本色（ToolTipForeground 的主要暗像素）
  · 边框色与粗细（BorderThickness=1）
  · **圆角半径**（CornerRadius=4）—— 逐行/逐列扫描**四角 6px 内**有没有背景色
  · 总尺寸（用于反推 padding：FluentFlyout 是 Border Padding=4 + ContentPresenter Margin=4）

⭐ 全部走像素，**不用**「绝对亮度差」当判据（那是背景的函数，Wiki 15 §9.2 记过这个坑）：
  这里比的是「tooltip 区域内部 vs 区域外部」两套像素本身。
"""

import ctypes
import ctypes.wintypes as wt
import io
import os
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
g = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

TTM_ACTIVATE = 1025
TTM_GETTOOLCOUNT = 1037
TTM_GETBUBBLESIZE = 1054
TTM_SETTOOLPOS = 1059          # WM_USER+35；windows-sys 未导出
SMTO_ABORTIFHUNG = 0x0002
SRCCOPY, CAPTUREBLT, DIB_RGB_COLORS = 0x00CC0020, 0x40000000, 0


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


for fn, res, args in [
    (u.GetWindowRect, wt.BOOL, [wt.HWND, ctypes.POINTER(RECT)]),
    (u.GetClassNameW, None, [wt.HWND, wt.LPWSTR, ctypes.c_int]),
    (u.IsWindowVisible, wt.BOOL, [wt.HWND]),
    (u.SendMessageTimeoutW, ctypes.c_uint, [wt.HWND, ctypes.c_uint, ctypes.c_size_t,
                                            ctypes.c_ssize_t, ctypes.c_uint,
                                            ctypes.c_uint, ctypes.POINTER(ctypes.c_size_t)]),
    (g.CreateCompatibleDC, wt.HDC, [wt.HDC]),
    (g.CreateCompatibleBitmap, wt.HBITMAP, [wt.HDC, ctypes.c_int, ctypes.c_int]),
    (g.SelectObject, wt.HGDIOBJ, [wt.HDC, wt.HGDIOBJ]),
    (g.BitBlt, ctypes.c_int, [wt.HDC] + [ctypes.c_int] * 4 + [wt.HDC] + [ctypes.c_int] * 2 + [ctypes.c_uint]),
    (g.GetDIBits, ctypes.c_int, [wt.HDC, wt.HBITMAP, ctypes.c_uint, ctypes.c_uint,
                                 ctypes.c_void_p, ctypes.POINTER(BI), ctypes.c_uint]),
]:
    fn.restype, fn.argtypes = res, args
u.GetDC.restype, u.GetDC.argtypes = wt.HDC, [wt.HWND]


def send(h, m, w=0, l=0):
    r = ctypes.c_size_t()
    ok = u.SendMessageTimeoutW(h, m, w, l, SMTO_ABORTIFHUNG, 2000, ctypes.byref(r))
    return (ok != 0, r.value)


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
    return buf


def find_tip():
    """找「**本进程那个**」tip 窗。

    ⛔ **必须以应用日志里的句柄为准**：`系统里有多个 tooltip_class32`
       （实测 13 个，含其它应用的），按「第一个注册数>0」挑会连到**别的应用**——
       实测踩到：日志里本应用是 4 条，探针读到 3 条（连错了窗）。
    """
    import glob
    import re
    import time as _t
    logs = sorted(glob.glob("D:/Code/PeriTray/src-tauri/target/debug/logs/*.log"),
                  key=os.path.getmtime)
    fresh = bool(logs) and (_t.time() - os.path.getmtime(logs[-1]) <= 60)
    if fresh:
        # ⛔ **新鲜日志里没匹配上时，绝不回退枚举**：回退必然连到**别的应用**的
        #   tip（系统里有多个，实测连到 3 条工具那个）⇒ 判据自身失效。
        #   「应用还没写到那一行」是**正常状态**，由调用方轮询重试。
        txt = io.open(logs[-1], encoding="utf-8", errors="replace").read()
        m = re.findall(r"tip 窗已建 hwnd=(0x[0-9a-fA-F]+)", txt)
        if not m:
            return None, 0
        h = int(m[-1], 16)
        if not u.IsWindow(h):
            return None, 0
        return h, send(h, TTM_GETTOOLCOUNT)[1]
    # 只有「压根没有新鲜日志」才回退枚举（用于脱离本脚本的单独诊断）
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, _):
        b = ctypes.create_unicode_buffer(256)
        u.GetClassNameW(h, b, 256)
        if b.value == "tooltips_class32":
            hits.append(h)
        return True

    u.EnumWindows(cb, 0)
    for h in hits:
        ok, n = send(h, TTM_GETTOOLCOUNT)
        if ok and n > 0:
            return h, n
    return None, 0


def main():
    # ⭐ **「找窗」也必须在轮询里**。
    #    探针与应用同时启动时，应用约 13s 后才写出「tip 窗已建」日志
    #    ⇒ 首轮 `find_tip` 拿不到我们的句柄、**回退到枚举**，
    #    而枚举会连到**别的应用**的 tip（实测连到 3 条工具的那个）
    #    ⇒ 之后 25s 都在盯一个无关窗口。判据自身失效。
    deadline = time.time() + 40
    tip = None
    n = 0
    while time.time() < deadline and tip is None:
        cand, cn = find_tip()
        if cand is not None and cn >= 3:      # 我们注册的是设备数（本机 3~4）
            tip, n = cand, cn
            break
        time.sleep(0.3)
    if not tip:
        print("❌ 40s 内未从应用日志里解析到 tip 句柄")
        return 1
    print("[P] tip=%#x  工具数=%d（句柄取自应用日志）" % (tip, n))

    r = RECT()
    shown = False
    while time.time() < deadline:
        if find_tip()[0] != tip:
            break
        u.GetWindowRect(tip, ctypes.byref(r))
        if (r.r - r.l) > 10 and bool(u.IsWindowVisible(tip)):
            shown = True
            break
        time.sleep(0.15)
    if not shown:
        print("❌ 轮询窗口内始终未见提示显示")
        return 1
    w, h = r.r - r.l, r.b - r.t
    print("[P] rect=(%d,%d)-(%d,%d)  %dx%d  visible=%s"
          % (r.l, r.t, r.r, r.b, w, h, bool(u.IsWindowVisible(tip))))
    ok, sz = send(tip, TTM_GETBUBBLESIZE)
    if ok:
        print("[P] TTM_GETBUBBLESIZE = %dx%d" % (sz & 0xFFFF, sz >> 16))

    # ⭐ **必须取「淡入动画结束」那一帧**。comctl32 的 tooltip 有淡入；
    #    在动画中采样 ⇒ 整窗半透明、上下表面色不一致、四角判据全部失效
    #    （实测「圆角检测 256/256 ⇒ 方角」就是这么来的，纯属误判）。
    #    做法：连抓若干帧、间隔拉开，取**最后一帧**。
    print("")
    print("采样序列（每帧的颜色种类数，用来看淡入是否收敛）：")
    frames = []
    for i in range(6):
        buf = grab(r.l, r.t, w, h)
        kinds = len({(buf[(y * w + x) * 4 + 2], buf[(y * w + x) * 4 + 1],
                      buf[(y * w + x) * 4]) for y in range(0, h, 2)
                     for x in range(0, w, 2)})
        print("   帧%d 颜色种类=%d" % (i, kinds))
        frames.append(buf)
        time.sleep(0.12)
    frames = [frames[-1]]

    def px(buf, x, y):
        i = (y * w + x) * 4
        return (buf[i + 2], buf[i + 1], buf[i])

    bg = px(frames[0], w // 2, 2)   # 顶部内 2px：应为表面色（无文字）
    print("")
    print("表面色(顶部内2px) = RGB%s" % (bg,))
    bg2 = px(frames[0], w // 2, h - 2)
    print("表面色(底部内2px) = RGB%s" % (bg2,))
    border_top = px(frames[0], w // 2, 0)
    border_left = px(frames[0], 0, h // 2)
    print("上边框(第0行)     = RGB%s" % (border_top,))
    print("左边框(第0列)     = RGB%s" % (border_left,))

    # 文本色：区域内最暗的、出现 >=3 次的颜色
    hist = {}
    for buf in frames[:1]:
        for y in range(h):
            for x in range(w):
                c = px(buf, x, y)
                hist[c] = hist.get(c, 0) + 1
    dark = sorted([kv for kv in hist.items() if sum(kv[0]) < 400], key=lambda kv: -kv[1])[:3]
    print("文本相关暗色      = %s" % (["RGB%s×%d" % (c, k) for c, k in dark],))

    # ── 圆角检测：先定位「白盒」，再量它的角 ────────────────────────
    # ⛔⛔ **判据已改三次，三次都错**，全部记录在此（这是本仓的典型坑）：
    #   ① 「非表面色」计数 ⇒ **边框像素**也算进去 ⇒ 矩形被判成有圆角。
    #   ② 「严格等于背景色」⇒ 圆角处是**混合色**（白+壁纸绿）⇒ 永远判成直角。
    #   ③ 「离表面/离背景就近分类」⇒ 混合色**被表面主导**（33 vs 416）⇒ 又判成直角。
    #   ④ 且窗口矩形**含阴影边距**，白盒是内缩的 ⇒ 量窗口的角量到了阴影区。
    # ✅ 正确判据：先**求白盒边界**（等于表面色的像素的包围盒），
    #    再判「角像素是否 ∉ {表面色, 边框色}」——直角 tooltip 的角**必然**是这两者之一，
    #    出现第三种值 ⇒ 那里被裁掉了。
    # ⚠️ 表面色必须在**垂直中线**取：y=3 落在圆角/抗锯齿带 ⇒ 会采到混合色
    surface = px(frames[0], w // 2, h // 2)
    border = px(frames[0], w // 2, 0)
    xs, ys = [], []
    for yy in range(h):
        for xx in range(w):
            if px(frames[0], xx, yy) == surface:
                xs.append(xx)
                ys.append(yy)
    print("")
    print("表面色 = RGB%s   边框色 = RGB%s" % (surface, border))
    if not xs:
        print("❌ 未找到表面色像素")
        return 1
    x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
    print("白盒包围盒 = x %d..%d, y %d..%d（窗口 %dx%d ⇒ 四周内缩 %d/%d/%d/%d）"
          % (x0, x1, y0, y1, w, h, x0, w - 1 - x1, y0, h - 1 - y1))

    allowed = {surface, border}
    corners = {"左上": (x0, y0), "右上": (x1, y0), "左下": (x0, y1), "右下": (x1, y1)}
    cut = 0
    for name, (cx, cy) in corners.items():
        c0 = px(frames[0], cx, cy)
        is_cut = c0 not in allowed
        cut += 1 if is_cut else 0
        print("   %s 角(白盒边界)=RGB%s %s"
              % (name, c0, "← 切角（圆角）" if is_cut else "← 实心"))
    print("   ⇒ %s" % ("**有圆角**" if cut >= 3 else "**直角，无圆角**"))

    # 圆角半径：沿白盒上边向右，找「不属于 {表面,边框}」的连续段
    run = 0
    for x in range(x0, min(x0 + 16, x1)):
        if px(frames[0], x, y0) not in allowed:
            run += 1
        else:
            break
    print("   白盒上边起连续「非表面/非边框」宽度 = %d px ⇒ 圆角半径 ≈ %d px" % (run, run))

    # 文字起始位置（反推 padding）
    first_dark = None
    for y in range(h):
        for x in range(w):
            c = px(frames[0], x, y)
            if sum(c) < 400:
                first_dark = (x, y)
                break
        if first_dark:
            break
    print("")
    print("最左/最上暗像素起点 = %s ⇒ 文本内缩 ≈ %s"
          % (first_dark, (first_dark[0], first_dark[1]) if first_dark else "无"))
    print("（FluentFlyout：Border Padding=4 + ContentPresenter Margin=4 ⇒ 理论内缩 8px）")
    return 0


if __name__ == "__main__":
    sys.exit(main())

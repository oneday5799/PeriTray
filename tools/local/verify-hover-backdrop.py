# -*- coding: utf-8 -*-
"""验收：hover 底衬（白色半透明 / 口径与 FluentFlyout 一致 / **仅 hover 时出现**）。

为什么这样设计：
  ⛔ 本机**无法注入鼠标输入**（SetCursorPos / SendInput / mouse_event 全无效，见 PLAYBOOK §E11.7）
     ⇒ 不能让光标去就窗口。
  ⛔ 也**不能**用 `SetWindowPos` 伪造：`draw_items` 每次重绘都按配置重新定位，挪过去的
     窗口会被立刻搬回原位。
  ✅ 反过来做 —— **让窗口来就光标**：`taskbar_custom_x` 是**合法配置**（拖拽落盘用的就是它），
     写进去后重启应用，窗口就定位到光标处 ⇒ hover 自然成立。全程不碰窗口 API。

判据为什么用「窗口内 vs 窗口外」的相对亮度：
  ⛔ 分层窗的 alpha 在屏幕合成后就没了 ⇒ 抓屏只能拿到**合成结果**，读不到 alpha。
  ⭐ 定量式：白色 alpha=153 叠加 ⇒ 结果 ≈ 0.4 × 背景 + 153（可精确核对，不是「变亮了」这种模糊话）。
"""

import ctypes
import ctypes.wintypes as wt
import glob
import os
import re
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.hoverbak")
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

WIDGET_CLASS = "PeriTrayTaskbarWidget"
WM_APP_REFRESH = 0x8000 + 1
WAIT_CURSOR_S = 240        # 等光标进入任务栏（多数时候它本来就在）
WAIT_LEAVE_S = 300         # 等光标离开 widget（本机无法注入鼠标 ⇒ 只能等用户移开）
ALPHA_LIGHT = 153          # FluentFlyout 浅色分支：255 × 0.6
# ⭐ 高度口径 = FluentFlyout 的 **40 DIP**（XAML `Height="40"`）按任务栏 DPI 换算。
#    ⛔ 别再硬编码 40：那是**物理像素**，125% 缩放下比 FluentFlyout 矮 10px。
DIP_WIDGET_H = 40

# ── DPI：必须在任何窗口/DC 操作之前设（本机 125%，否则拿到虚拟化坐标）──
u = ctypes.WinDLL("user32", use_last_error=True)
gdi = ctypes.WinDLL("gdi32", use_last_error=True)

u.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
u.SetProcessDpiAwarenessContext.restype = wt.BOOL
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))   # PER_MONITOR_AWARE_V2

u.FindWindowW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p]
u.FindWindowW.restype = ctypes.c_void_p
u.FindWindowExW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_wchar_p]
u.FindWindowExW.restype = ctypes.c_void_p
u.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(wt.RECT)]
u.GetWindowRect.restype = wt.BOOL
u.GetCursorPos.argtypes = [ctypes.POINTER(wt.POINT)]
u.GetCursorPos.restype = wt.BOOL
u.IsWindow.argtypes = [ctypes.c_void_p]
u.IsWindow.restype = wt.BOOL
u.IsWindowVisible.argtypes = [ctypes.c_void_p]
u.IsWindowVisible.restype = wt.BOOL
u.GetParent.argtypes = [ctypes.c_void_p]
u.GetParent.restype = ctypes.c_void_p
u.ClientToScreen.argtypes = [ctypes.c_void_p, ctypes.POINTER(wt.POINT)]
u.ClientToScreen.restype = wt.BOOL
u.PostMessageW.argtypes = [ctypes.c_void_p, wt.UINT, ctypes.c_size_t, ctypes.c_ssize_t]
u.PostMessageW.restype = wt.BOOL
# ⚠️ GetDC / ReleaseDC 在 **user32**（不是 gdi32）
u.GetDC.argtypes = [ctypes.c_void_p]
u.GetDC.restype = ctypes.c_void_p
u.ReleaseDC.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
u.ReleaseDC.restype = ctypes.c_int
# ⭐ GetDpiForWindow 在 **user32**；用**任务栏**的 DPI（widget 是它的子窗）
u.GetDpiForWindow.argtypes = [ctypes.c_void_p]
u.GetDpiForWindow.restype = wt.UINT

gdi.CreateCompatibleDC.argtypes = [ctypes.c_void_p]
gdi.CreateCompatibleDC.restype = ctypes.c_void_p
gdi.CreateCompatibleBitmap.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
gdi.CreateCompatibleBitmap.restype = ctypes.c_void_p
gdi.SelectObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
gdi.SelectObject.restype = ctypes.c_void_p
gdi.DeleteObject.argtypes = [ctypes.c_void_p]
gdi.DeleteObject.restype = wt.BOOL
gdi.DeleteDC.argtypes = [ctypes.c_void_p]
gdi.DeleteDC.restype = wt.BOOL
gdi.BitBlt.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                       ctypes.c_void_p, ctypes.c_int, ctypes.c_int, wt.DWORD]
gdi.BitBlt.restype = wt.BOOL
gdi.GetDIBits.argtypes = [ctypes.c_void_p, ctypes.c_void_p, wt.UINT, wt.UINT, ctypes.c_void_p,
                          ctypes.c_void_p, wt.UINT]
gdi.GetDIBits.restype = ctypes.c_int

SRCCOPY = 0x00CC0020
CAPTUREBLT = 0x40000000     # ⛔ 抓分层窗必须加


class BITMAPINFOHEADER(ctypes.Structure):
    _fields_ = [("biSize", wt.DWORD), ("biWidth", ctypes.c_long), ("biHeight", ctypes.c_long),
                ("biPlanes", wt.WORD), ("biBitCount", wt.WORD), ("biCompression", wt.DWORD),
                ("biSizeImage", wt.DWORD), ("biXPelsPerMeter", ctypes.c_long),
                ("biYPelsPerMeter", ctypes.c_long), ("biClrUsed", wt.DWORD),
                ("biClrImportant", wt.DWORD)]


RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def note(msg):
    print(f"  · {msg}")
    sys.stdout.flush()


# ── 抓屏 ────────────────────────────────────────────────────────────

def grab(left, top, w, h):
    """抓屏幕一块区域，返回 top-down 的 BGRA **bytes**。"""
    hdc = u.GetDC(None)
    mem = gdi.CreateCompatibleDC(hdc)
    bmp = gdi.CreateCompatibleBitmap(hdc, w, h)
    old = gdi.SelectObject(mem, bmp)
    gdi.BitBlt(mem, 0, 0, w, h, hdc, left, top, CAPTUREBLT | SRCCOPY)
    bih = BITMAPINFOHEADER()
    bih.biSize = ctypes.sizeof(BITMAPINFOHEADER)
    bih.biWidth = w
    bih.biHeight = -h          # 负值 = top-down
    bih.biPlanes = 1
    bih.biBitCount = 32
    bih.biCompression = 0
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bih), 0)
    gdi.SelectObject(mem, old)
    gdi.DeleteObject(bmp)
    gdi.DeleteDC(mem)
    u.ReleaseDC(None, hdc)
    # ⛔ 必须取 `.raw`：`create_string_buffer` 的**下标**返回的是长度 1 的 bytes，
    #    不是 int ⇒ `0.299 * b'\x12'` 会报 can't multiply sequence by non-int。
    return buf.raw


def rgb_at(data, w, x, y):
    i = (y * w + x) * 4
    return data[i + 2], data[i + 1], data[i]      # BGRA -> (r,g,b)


def lum(rgb):
    r, g, b = rgb
    return 0.299 * r + 0.587 * g + 0.114 * b


# ── 窗口 ────────────────────────────────────────────────────────────

def taskbar_hwnd():
    return u.FindWindowW("Shell_TrayWnd", None)


def expect_h():
    """FluentFlyout 的底衬高度：40 DIP × (任务栏 DPI / 96)。"""
    tb = taskbar_hwnd()
    dpi = u.GetDpiForWindow(tb) if tb else 96
    dpi = dpi or 96
    return round(DIP_WIDGET_H * dpi / 96.0), dpi


def widget_hwnd():
    tray = taskbar_hwnd()
    return u.FindWindowExW(tray, None, WIDGET_CLASS, None) if tray else 0


def rect_of(hwnd):
    r = wt.RECT()
    if not hwnd or not u.GetWindowRect(hwnd, ctypes.byref(r)):
        return None
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def cursor():
    pt = wt.POINT()
    if not u.GetCursorPos(ctypes.byref(pt)):
        return None
    return (pt.x, pt.y)


def cursor_in(rect):
    if not rect:
        return False
    pt = cursor()
    if not pt:
        return False
    left, top, w, h = rect
    return left <= pt[0] < left + w and top <= pt[1] < top + h


def client_left(hwnd):
    pt = wt.POINT(0, 0)
    u.ClientToScreen(u.GetParent(hwnd), ctypes.byref(pt))
    return pt.x


# ── 配置读写（⛔ 必须是顶层键：裸键会落进最近一个 [table]）──────────

def drop_config_line(key):
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    pat = re.compile(rf"^{re.escape(key)}\s*=\s*\[", re.M)
    while True:
        m = pat.search(txt)
        if not m:
            break
        j = m.end() - 1
        depth = 0
        while j < len(txt):
            if txt[j] == "[":
                depth += 1
            elif txt[j] == "]":
                depth -= 1
                if depth == 0:
                    j += 1
                    break
            j += 1
        while j < len(txt) and txt[j] != "\n":
            j += 1
        if j < len(txt):
            j += 1
        txt = txt[:m.start()] + txt[j:]
    txt = re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M)
    open(CFG, "w", encoding="utf-8").write(txt)


def set_config_line(key, line):
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    txt = re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M)
    open(CFG, "w", encoding="utf-8").write(line + "\n" + txt)


def drop_pins():
    """清空 pin 列表（TOML **数组表**）。

    ⛔⛔ `pinned_taskbar_devices` 在 config.toml 里是 `[[pinned_taskbar_devices]]` **数组表**，
       `drop_config_line()` 只认 `key = [...]`（内联数组）与 `key = ...`（标量）两种形态
       ⇒ 对数组表**匹配不到、静默无效**（pin 一个不少，widget 只是更宽）。
       必须按 TOML 语义「表头切换上下文」删：从该表头起，到下一个 `[`/`[[` 表头之前的
       全部行（字段 / 注释 / 空行）都丢掉。
    """
    lines = open(CFG, encoding="utf-8", errors="replace").read().splitlines()
    out, skipping = [], False
    for ln in lines:
        s = ln.strip()
        if s.startswith("["):
            skipping = (s == "[[pinned_taskbar_devices]]")
            if skipping:
                continue
        elif skipping:
            continue
        out.append(ln)
    open(CFG, "w", encoding="utf-8").write("\n".join(out) + "\n")


def log_text():
    logs = sorted(glob.glob(os.path.join(WD, "logs", "*.log")), key=os.path.getmtime)
    return open(logs[-1], "r", encoding="utf-8", errors="replace").read() if logs else ""


def cdp(expr, timeout=90):
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


PIN = ("(async () => {"
       "  const inv = window.__TAURI__.core.invoke;"
       "  const devs = await inv('get_taskbar_devices');"
       "  const sel = await inv('get_selectable_devices');"
       "  const pinned = new Set((sel || []).filter(d => d.pinned).map(d => d.key));"
       "  const audio = (devs || []).find(d => d.audio_device_id && d.volume != null);"
       "  const plain = (devs || []).find(d => !d.audio_device_id && d.battery != null);"
       "  let pick = [audio, plain].filter(Boolean);"
       "  if (pick.length < 2) { pick = (devs || []).slice(0, 2); }"
       "  const added = [];"
       "  for (const d of pick) {"
       "    if (pinned.has(d.key)) continue;"
       "    await inv('toggle_pinned_taskbar_device', { key: d.key, fallback: null, alias: null });"
       "    added.push(d.name);"
       "  }"
       "  return JSON.stringify({pick: pick.map(d => d.name), added}); })()")


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


def launch():
    env = dict(os.environ)
    env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    return subprocess.Popen([EXE], cwd=WD, env=env)


def start_and_find(pin=False, wait=9):
    """启动应用并等 widget 出现。返回 (proc, hwnd)。"""
    proc = launch()
    time.sleep(wait)
    hwnd = 0
    for _ in range(3):
        if pin:
            print("  pinned:", cdp(PIN))
        for _ in range(20):
            hwnd = widget_hwnd()
            if hwnd:
                break
            time.sleep(0.5)
        if hwnd:
            break
    time.sleep(1.5)
    return proc, hwnd


# ── 采样：窗口内底衬点 vs 同一行的窗口外背景点 ─────────────────────

def row_lift(data, bw, out, y, x_in):
    """同一行上「窗口内 vs 窗口外」的亮度与差值。

    ⛔ 两个约束缺一不可（各自踩过一次坑，见 `sample` 的文档）：
       ① **同行**（`y` 相同）—— 任务栏的搜索框高亮区只占 y≈11~45，跨行比较会把它算进来；
       ② **横向相邻**（`x_in` 必须小到落在左侧内边距列）—— 相距 141px 时两点背景本就差 21.5。
    """
    inside = lum(rgb_at(data, bw, out + x_in, y))
    outside = lum(rgb_at(data, bw, 0, y))
    return inside, outside, inside - outside


def wait_content(hwnd, min_w=40, timeout=25):
    """等 widget **真的画出内容**（宽度 > `min_w`），返回最新 rect。

    ⛔⛔ 刚勾选设备时，设备列表（WMI，600ms+）还没回来 ⇒ widget 会先以**极小宽度**出现
       （实测 **16px**）⇒ 此时采样必然假红。修好「清 pin」后 pin 真的是从 0 开始，
       这条时序才暴露出来（此前配置里留着旧 pin，widget 一建出来就有内容，掩盖了它）。
    """
    t0 = time.time()
    while time.time() - t0 < timeout:
        r = rect_of(hwnd)
        if r and r[2] > min_w:
            return r
        time.sleep(0.5)
    return rect_of(hwnd)


def sample(rect):
    """返回 (mid, up, dn, corner)，每项 = (内, 外, 差值)。

    ⛔⛔ 采样点**必须落在窗口的左侧内边距列**（local x = 2；`PAD_X = 6` ⇒ 该列恒为空白），
       且「内 / 外」两点**横向只隔 6px**。踩过的两个坑：
         · 第一版拿「顶行/底行**中部**」当纯底衬区 ⇒ 撞上**窗口内容**（图标自 y=4 起，
           文字在 y=4~36）⇒ 未 hover 也读出 247.4（内容本身就很亮）。
         · 第二版改成「同行比较」，但内外仍相距 **141px** ⇒ 撞上**任务栏背景本身的横向不均**。
       取证（`probe-taskbar-profile.py`）：本机任务栏左侧有一段**搜索框高亮区**
       （x≈80~310、y≈11~45，亮度 **247.7**），而 x=16 处是任务栏底色 **225.9**。
       窗口在 `custom_x=20` 时，窗口内 x=157 落在高亮区、窗口外 x=16 落在底色上
       ⇒ 未 hover 也读出 **+21.5** 的假抬升。**改成横向相邻后，同一对全程只差 ≤2.0**。
    """
    left, top, w, h = rect
    out = 4
    data = grab(left - out, top, w + out, h)
    bw = w + out
    mid = row_lift(data, bw, out, h // 2, 2)        # 左侧内边距列（纯底衬区）
    up = row_lift(data, bw, out, 8, 2)              # 同列靠上（仍在圆角之内：6 ≤ y < h-6）
    dn = row_lift(data, bw, out, h - 8, 2)          # 同列靠下
    corner = row_lift(data, bw, out, 0, 0)          # 左上角（圆角之外）
    return mid, up, dn, corner


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml，先手动跑一次应用生成配置")
        return 1
    open(CFG_BAK, "w", encoding="utf-8").write(open(CFG, encoding="utf-8").read())
    proc = None
    try:
        print("=== 准备：unlocked + 勾选设备 ===")
        kill()
        drop_pins()
        drop_config_line("taskbar_custom_x")
        set_config_line("taskbar_position_locked", "taskbar_position_locked = false")
        proc, hwnd = start_and_find(pin=True)
        check("P 前提：widget 存在且可见",
              bool(hwnd) and bool(u.IsWindow(hwnd)) and bool(u.IsWindowVisible(hwnd)),
              f"hwnd={hwnd:#x}" if hwnd else "未出现 widget 窗口")
        if not hwnd:
            return 1
        tb = rect_of(taskbar_hwnd())
        rect = rect_of(hwnd)
        exp_h, dpi = expect_h()
        note(f"任务栏 = {tb}（DPI {dpi}）")
        note(f"widget = {rect}（高度 {rect[3]}）")
        note(f"光标 = {cursor()}（在 widget 内 = {cursor_in(rect)}）")
        check(f"P2 widget 高度 = {exp_h}（= FluentFlyout 的 40 DIP × {dpi}/96）",
              rect[3] == exp_h, f"实测 {rect[3]}（期望 {exp_h}）")

        # ── 阶段 0：确保 widget 已画出内容、且左侧有采样余量 ────────
        if rect[2] <= 40:
            note(f"widget 宽度仅 {rect[2]}px ⇒ 内容尚未就绪（设备列表还没回来），等它变宽…")
            rect = wait_content(hwnd)
            note(f"widget = {rect}（宽 {rect[2]}）")

        # ── 阶段 1：基线（未 hover ⇒ 不该有底衬）────────────────────
        print("\n=== 阶段 1：基线（未 hover）===")
        # ⛔ 两种情况都必须挪窗：
        #    ① 光标就在 widget 上（会触发 hover）；
        #    ② widget 贴到 x < 8 —— 采样要取「窗口左外侧 4px」当背景，
        #       贴到 x=0 会采到**屏幕外**（实测 外=0.0 ⇒ A1/A2 假红，纯脚本自伤）。
        if cursor_in(rect) or rect[0] < 8:
            why = ("光标当前就在 widget 上" if cursor_in(rect)
                   else f"widget 贴到 x={rect[0]}，左侧没有采样余量")
            note(f"{why} ⇒ 先把窗口挪开（写 custom_x + 重启）")
            pt = cursor()
            # ⛔ 留 20px 余量：采样要取「窗口左外侧 4px」当背景，贴到 x=0 会采到屏幕外
            #    （实测外=0.0 ⇒ 判据假红，纯属脚本自伤）。
            # ⭐ **优先放最左端**：实测 x=0~40 全高都是平坦的任务栏底色（~225.9），
            #    是采样最可靠的位置；只有光标本身就待在这一带时才退到最右端。
            if not (tb[0] <= pt[0] < tb[0] + rect[2] + 20):
                far = tb[0] + 20
            else:
                far = tb[0] + tb[2] - rect[2] - 20
            set_config_line("taskbar_custom_x", f"taskbar_custom_x = {far}")
            kill()
            proc, hwnd = start_and_find()
            rect = wait_content(hwnd)
            note(f"widget 已挪到 {rect}（光标在 widget 内 = {cursor_in(rect)}）")
        check("A 前提：光标不在 widget 上", not cursor_in(rect), f"光标={cursor()}")
        mid, up, dn, corner = sample(rect)
        check("A1 未 hover ⇒ 无底衬（窗口内亮度 == 同行窗口外背景）", abs(mid[2]) < 6.0,
              f"内={mid[0]:.1f} 外={mid[1]:.1f} 差={abs(mid[2]):.1f}（阈值 6）")
        check("A2 未 hover ⇒ 上/下两行（左侧内边距列）也无底衬",
              abs(up[2]) < 6.0 and abs(dn[2]) < 6.0,
              f"上 差={abs(up[2]):.1f}（内={up[0]:.1f} 外={up[1]:.1f}）"
              f" 下 差={abs(dn[2]):.1f}（内={dn[0]:.1f} 外={dn[1]:.1f}）")
        check("A3 日志此时**没有** hover 记录", "hover: 进入" not in log_text(),
              "若已有记录说明底衬逻辑被别的东西触发了")

        # ── 阶段 2：构造 hover（把窗口挪到光标下）────────────────────
        print(f"\n=== 阶段 2：构造 hover（窗口就光标，最多等 {WAIT_CURSOR_S}s）===")
        hovered = False
        t0 = time.time()
        attempt = 0
        while time.time() - t0 < WAIT_CURSOR_S:
            pt = cursor()
            # ⛔ tb 是 (left, top, W, H) —— y 的右界必须写 tb[1] + tb[3]，
            #    写成 tb[3] 会让判据恒假（实测：脚本空转到超时，日志一句不输出）。
            if pt and tb[0] <= pt[0] < tb[0] + tb[2] and tb[1] <= pt[1] < tb[1] + tb[3]:
                attempt += 1
                cw = rect_of(hwnd)[2]
                target = max(0, min(pt[0] - cw // 2 - client_left(hwnd), tb[2] - cw))
                note(f"第 {attempt} 次：光标 {pt} ⇒ custom_x={target}")
                set_config_line("taskbar_custom_x", f"taskbar_custom_x = {target}")
                kill()
                proc, hwnd = start_and_find()
                rect = wait_content(hwnd)
                if cursor_in(rect):
                    hovered = True
                    break
                note(f"  重启后 widget={rect}，光标仍不在其上，重试…")
            else:
                time.sleep(0.5)
        check("B 观测到光标落在 widget 上（hover 成立）", hovered,
              f"尝试 {attempt} 次" if hovered else f"{WAIT_CURSOR_S}s 内光标未进入任务栏")
        if not hovered:
            print("\n!! 未构造出 hover —— 底衬相关判据**未验证**（不是失败）")
            return 2
        time.sleep(1.0)     # 让 50ms 轮询 + 重绘跑完

        rect = wait_content(hwnd)
        mid, up, dn, corner = sample(rect)
        check("G 日志出现「hover: 进入 ⇒ 显示底衬」", "hover: 进入" in log_text(), "")

        expect = (1 - ALPHA_LIGHT / 255.0) * mid[1] + (ALPHA_LIGHT / 255.0) * 255.0
        err = abs(mid[0] - expect)
        check("C 底衬出现且**定量**吻合 白 alpha=153 合成式", err <= 8.0,
              f"外={mid[1]:.1f} 内={mid[0]:.1f} 期望={expect:.1f} 误差={err:.1f}（阈值 8）")
        # ⚠️ 阈值 8 而不是 20：本机**浅色任务栏**（背景亮度 ~232）下，白 alpha=153 叠加
        #    只把亮度抬到 ~246（0.4×232+153）⇒ 实际抬升约 14。这是 FluentFlyout 的
        #    原生行为（浅色主题下 60% 白在浅色底上本就不明显），不是缺陷。
        check("C2 底衬是**提亮**而非压暗", mid[0] > mid[1] + 8, f"抬升 {mid[0] - mid[1]:+.1f}")
        # ⭐ 用**抬升量**比较三行（而不是比较绝对亮度）：抬升量把「背景本身横向/纵向不均」
        #    约掉了，判据才与任务栏内容无关。
        check("D 底衬覆盖窗口**全高**（上/中/下三行同为底衬）",
              up[2] > 8 and dn[2] > 8 and abs(up[2] - mid[2]) < 8 and abs(dn[2] - mid[2]) < 8,
              f"抬升 上={up[2]:+.1f} 中={mid[2]:+.1f} 下={dn[2]:+.1f}")
        # ⚠️ 阈值 8：本机**浅色任务栏**（背景 ~230）下白 alpha=153 只把亮度抬到 ~245
        #    （0.4×230+153=245）⇒ 角落与中心的实际差只有 ~15。这是 FluentFlyout 的原生
        #    行为（浅色主题下 60% 白在浅色底上本就不明显），不是缺陷 —— 阈值必须按实测定。
        check("E 左上角在圆角之外（不是底衬）", corner[2] < 8.0,
              f"角抬升={corner[2]:+.1f}（角应≈同行背景 ⇒ 抬升≈0）")

        # ── 阶段 3：光标离开 ⇒ 底衬消失 ─────────────────────────────
        print(f"\n=== 阶段 3：等光标离开（最多 {WAIT_LEAVE_S}s，请把鼠标移开）===")
        t0 = time.time()
        left_ok = False
        while time.time() - t0 < WAIT_LEAVE_S:
            if not cursor_in(rect_of(hwnd) or (0, 0, 0, 0)):
                left_ok = True
                break
            time.sleep(0.3)
        if not left_ok:
            # ⛔ 本机**无法注入鼠标**（SetCursorPos / SendInput / mouse_event 全无效，
            #    见 PLAYBOOK §E11.7）⇒ 这条只能靠用户把鼠标移开。
            #    ⛔ 不要用「挪窗」来伪造：那会重启进程 ⇒ 新日志里**不会**出现
            #    「hover: 离开」（新进程的 `HOVERED` 本来就是 false，`want == HOVERED`
            #    ⇒ 不产生日志）⇒ G2 会变成误导性的 FAIL。
            note(f"{WAIT_LEAVE_S}s 内光标未离开 ⇒ F / F2 / G2 **未验证**（非失败）")
            print("\n=== 结果 ===")
            ok = sum(1 for _, o, _ in RESULTS if o)
            for n, o, d in RESULTS:
                if not o:
                    print(f"  ❌ {n}  —— {d}")
            print(f"{ok}/{len(RESULTS)} 通过（F/F2/G2 未验证）")
            return 0 if ok == len(RESULTS) else 1

        check("F 观测到光标离开 widget", True, f"等待 {time.time() - t0:.1f}s")
        time.sleep(1.0)
        mid2 = sample(rect_of(hwnd))[0]
        check("F2 离开后底衬消失（亮度回落）", abs(mid2[2]) < 6.0,
              f"内={mid2[0]:.1f} 外={mid2[1]:.1f} 差={abs(mid2[2]):.1f}")
        check("G2 日志出现「hover: 离开 ⇒ 隐藏底衬」", "hover: 离开" in log_text())

        print("\n=== 结果 ===")
        ok = sum(1 for _, o, _ in RESULTS if o)
        for n, o, d in RESULTS:
            if not o:
                print(f"  ❌ {n}  —— {d}")
        print(f"{ok}/{len(RESULTS)} 通过")
        return 0 if ok == len(RESULTS) else 1
    finally:
        if proc:
            kill()
        if os.path.exists(CFG_BAK):
            open(CFG, "w", encoding="utf-8").write(open(CFG_BAK, encoding="utf-8").read())
            os.remove(CFG_BAK)
        print("（已还原 config.toml 并结束进程）")


if __name__ == "__main__":
    sys.exit(main())

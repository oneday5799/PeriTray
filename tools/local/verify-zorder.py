"""任务栏 widget **Z 序维护**的真机验收（B3-A 里程碑 6）。

背景（真机实测，2026-09-25）：
  · `SetParent` 会把窗口放到**兄弟 Z 序最顶** —— 但那是**建窗顺序的副产品**，不是契约；
  · 实测：用同款形态（popup → 改样式 → `SetParent`）建一个兄弟窗，它落第 0 位、
    **我们的 widget 被挤到第 1 位**（`probe-zorder-race2.py`）；
  · 另一条建窗路线（`CreateWindowExW` 直接以任务栏为父）落**最底**；
  · ⇒ 可见性不能依赖「挂载顺序」，必须**周期性重申**（参考实现 StockBar 的
    「约 2 秒维护重贴 Z 序」正是干这个）。

判据（每条都可证伪）：
  P. **前提**：widget 已挂载、且「入侵者」确实压在我们之上（Z 序号更小 **且**
     像素层面确实遮住我们）。⛔ 不做这一步，「恢复」可能只是「入侵者根本没上去」的假象。
  A. **维护把 Z 序拉回来**：入侵者上来后，≤ 8s 内 widget 回到第 0 位。
  B. **像素层面也恢复**：恢复后 widget 区域内不再是入侵者的品红色。
  G. 日志留下「Z 序被后来者压住 ⇒ 已重申到兄弟最顶」（可追溯）。
  C. **可证伪**：注入「禁用维护期重申」⇒ A 必须**转红**（widget 停在非 0 位）。
  D. **Explorer 重建路径**（用 `WM_CLOSE` 销毁 widget 模拟「被连带销毁」）：
       · 无修复（干净构建）⇒ 10s 内**不**恢复（30s 慢节拍）；
       · 有修复（注入「任务栏句柄变了」）⇒ ≤ 5s 恢复。
"""

import ctypes
import ctypes.wintypes as w
import glob
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

SRC = Path(r"D:\Code\PeriTray\src-tauri\src\taskbar_widget.rs")
WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.zbak")
CARGO = r"C:\Users\Oneday\.cargo\bin\cargo.exe"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

# ── 注入 A：禁用**维护期**的 Z 序重申（挂载期的那次保留）──
INJ_RAISE = (
    "禁用维护期的 Z 序重申",
    """                if plan.raise {
                    let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
                    if !hwnd.is_null() {""",
    """                if plan.raise {
                    let hwnd = WIDGET_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
                    if false && !hwnd.is_null() {""",
)

# ── 注入 B：把「任务栏句柄变了」恒置真（env 门控），用于验证 Explorer 重建的快路径 ──
INJ_CHANGED = (
    "任务栏句柄变化判据恒真（env 门控）",
    "                let changed = taskbar != MOUNTED_TASKBAR.load(Ordering::SeqCst);",
    "                let changed = taskbar != MOUNTED_TASKBAR.load(Ordering::SeqCst)\n"
    '                    || std::env::var("PM_TEST_FORCE_REBUILD").is_ok();',
)

u = ctypes.WinDLL("user32", use_last_error=True)
gdi = ctypes.WinDLL("gdi32", use_last_error=True)
k32 = ctypes.WinDLL("kernel32", use_last_error=True)

# ⛔ 必须显式声明 argtypes：不声明时 ctypes 会把 64 位句柄**截断成 32 位**
#    （本机句柄值恰好都不大，所以不声明也能"跑通" —— 正是这种"跑通了"最危险）。
HWND, HANDLE, HDC, HGDIOBJ, HINSTANCE = (
    w.HANDLE, w.HANDLE, w.HANDLE, w.HANDLE, w.HANDLE,
)
u.FindWindowW.argtypes = [w.LPCWSTR, w.LPCWSTR]
u.FindWindowW.restype = HWND
u.FindWindowExW.argtypes = [HWND, HWND, w.LPCWSTR, w.LPCWSTR]
u.FindWindowExW.restype = HWND
u.GetWindow.argtypes = [HWND, ctypes.c_uint]
u.GetWindow.restype = HWND
u.GetParent.argtypes = [HWND]
u.GetParent.restype = HWND
u.SetParent.argtypes = [HWND, HWND]
u.SetParent.restype = HWND
u.GetClassNameW.argtypes = [HWND, w.LPWSTR, ctypes.c_int]
u.GetWindowRect.argtypes = [HWND, ctypes.POINTER(w.RECT)]
u.WindowFromPoint.argtypes = [w.POINT]
u.WindowFromPoint.restype = HWND
u.ScreenToClient.argtypes = [HWND, ctypes.POINTER(w.POINT)]
u.IsWindow.argtypes = [HWND]
u.IsWindowVisible.argtypes = [HWND]
u.GetWindowLongPtrW.argtypes = [HWND, ctypes.c_int]
u.GetWindowLongPtrW.restype = ctypes.c_ssize_t
u.SetWindowLongPtrW.argtypes = [HWND, ctypes.c_int, ctypes.c_ssize_t]
u.SetWindowLongPtrW.restype = ctypes.c_ssize_t
u.SetWindowPos.argtypes = [HWND, HWND, ctypes.c_int, ctypes.c_int,
                           ctypes.c_int, ctypes.c_int, ctypes.c_uint]
u.ShowWindow.argtypes = [HWND, ctypes.c_int]
u.DestroyWindow.argtypes = [HWND]
u.PostMessageW.argtypes = [HWND, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
u.GetDC.argtypes = [HWND]
u.GetDC.restype = HDC
u.ReleaseDC.argtypes = [HWND, HDC]
u.GetSystemMetrics.argtypes = [ctypes.c_int]
gdi.GetPixel.argtypes = [HDC, ctypes.c_int, ctypes.c_int]
gdi.GetPixel.restype = ctypes.c_uint32
gdi.GetStockObject.argtypes = [ctypes.c_int]
gdi.GetStockObject.restype = HANDLE
gdi.CreateCompatibleDC.argtypes = [HDC]
gdi.CreateCompatibleDC.restype = HDC
gdi.DeleteDC.argtypes = [HDC]
gdi.SelectObject.argtypes = [HDC, HGDIOBJ]
gdi.SelectObject.restype = HGDIOBJ
gdi.DeleteObject.argtypes = [HGDIOBJ]
u.DefWindowProcW.argtypes = [HWND, ctypes.c_uint, ctypes.c_size_t, ctypes.c_ssize_t]
u.DefWindowProcW.restype = ctypes.c_ssize_t

try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    u.SetProcessDPIAware()

GW_CHILD, GW_HWNDNEXT, GW_HWNDPREV = 5, 2, 3
GWL_STYLE, GWL_EXSTYLE = -16, -20
WS_POPUP, WS_CHILD, WS_VISIBLE = 0x80000000, 0x40000000, 0x10000000
WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_NOACTIVATE = 0x80000, 0x80, 0x08000000
SWP_NOMOVE, SWP_NOSIZE, SWP_NOACTIVATE, SWP_NOZORDER = 0x0002, 0x0001, 0x0010, 0x0004
HWND_TOP = 0
ULW_ALPHA = 2
DIB_RGB_COLORS = 0
WM_CLOSE = 0x0010
OURS = "PeriTrayTaskbarWidget"
INTRUDER = "PeriTrayZOrderIntruder"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'✅' if ok else '❌'} {name}" + (f"  —— {detail}" if detail else ""))


# ── 基础 ──────────────────────────────────────────────────────────

def cls_of(h):
    b = ctypes.create_unicode_buffer(256)
    u.GetClassNameW(h, b, 256)
    return b.value


def taskbar():
    return u.FindWindowW("Shell_TrayWnd", None)


def widget_hwnd():
    t = taskbar()
    return u.FindWindowExW(t, None, OURS, None) if t else None


def rect_of(h):
    r = w.RECT()
    u.GetWindowRect(h, ctypes.byref(r))
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def zsiblings():
    """任务栏子窗，**从顶到底**。"""
    t = taskbar()
    out, h = [], u.GetWindow(t, GW_CHILD)
    while h and len(out) < 200:
        out.append(h)
        h = u.GetWindow(h, GW_HWNDNEXT)
    return out


def zindex_of(cls):
    for i, h in enumerate(zsiblings()):
        if cls_of(h) == cls:
            return i
    return None


def magenta_ratio(rect, step=16):
    """⚠️ **仅作信息输出，不能当判据**。

    ⛔ 踩过的坑：widget 大部分像素 `alpha = 0`（分层窗按 alpha 合成），
       下方入侵者的不透明品红**本来就会透过来** ⇒ 即使 widget 稳稳在 Z 序最顶，
       这个值依然是 0.94。把它当「是否被遮住」的判据 = **假红**（第一版就这样误判）。
       真正的判据是 Z 序号与 `owner_at`（命中测试归属）。
    """
    left, top, width, height = rect
    y = top + height // 2
    scr = u.GetDC(None)
    hit = total = 0
    for dx in range(0, width, step):
        c = gdi.GetPixel(scr, left + dx, y)
        if c != 0xFFFFFFFF:  # CLR_INVALID
            total += 1
            r, g, b = c & 0xFF, (c >> 8) & 0xFF, (c >> 16) & 0xFF
            if r > 200 and g < 60 and b > 200:
                hit += 1
    u.ReleaseDC(None, scr)
    return (hit / total) if total else -1.0


def owner_at(rect):
    """`rect` **中心点**的真实命中归属（类名）。

    ⭐ 这才是「谁在这个点上、谁会收到鼠标」的判据：分层窗按 alpha 做命中测试，
       不透明像素才会命中 ⇒ 入侵者在上面时归入侵者，被重申拉回后归我们。
    """
    left, top, width, height = rect
    h = u.WindowFromPoint(w.POINT(left + width // 2, top + height // 2))
    return cls_of(h) if h else None


def log_text():
    logs = sorted(glob.glob(os.path.join(WD, "logs", "*.log")), key=os.path.getmtime)
    return open(logs[-1], "r", encoding="utf-8", errors="replace").read() if logs else ""


def cdp(expr, timeout=60):
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


def launch(extra_env=None):
    env = dict(os.environ)
    env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    if extra_env:
        env.update(extra_env)
    return subprocess.Popen([EXE], cwd=WD, env=env)


# ── 配置读写（与 verify-widget-drag.py 同款，含两个已踩过的 TOML 坑）──

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
    m = re.search(r"^\[", txt, re.M)
    txt = (txt.rstrip("\n") + "\n" + line + "\n") if m is None \
        else (txt[:m.start()] + line + "\n" + txt[m.start():])
    open(CFG, "w", encoding="utf-8").write(txt)


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
       "    await inv('toggle_pinned_taskbar_device',"
       "      { key: d.key, fallback: null, alias: null });"
       "    added.push(d.name);"
       "  }"
       "  return JSON.stringify({pick: pick.map(d => d.name), added}); })()")


def start_app(label, extra_env=None):
    print(f"\n=== 启动（{label}）===")
    drop_config_line("pinned_taskbar_devices")
    drop_config_line("taskbar_custom_x")
    set_config_line("taskbar_position_locked", "taskbar_position_locked = false")
    proc = launch(extra_env)
    time.sleep(9)
    hwnd = None
    for i in range(3):
        print(f"  pinned[{i}]:", cdp(PIN))
        for _ in range(20):
            hwnd = widget_hwnd()
            if hwnd:
                break
            time.sleep(0.5)
        if hwnd:
            break
    time.sleep(2)
    print(f"  widget hwnd = {hwnd if not hwnd else hex(hwnd)}")
    return proc, hwnd


# ── 入侵者：同款形态（popup → 改样式 → SetParent）+ 不透明品红分层填充 ──

class BMIH(ctypes.Structure):
    _fields_ = [("biSize", ctypes.c_uint32), ("biWidth", ctypes.c_int32),
                ("biHeight", ctypes.c_int32), ("biPlanes", ctypes.c_uint16),
                ("biBitCount", ctypes.c_uint16), ("biCompression", ctypes.c_uint32),
                ("biSizeImage", ctypes.c_uint32), ("biXPelsPerMeter", ctypes.c_int32),
                ("biYPelsPerMeter", ctypes.c_int32), ("biClrUsed", ctypes.c_uint32),
                ("biClrImportant", ctypes.c_uint32)]


class BMI(ctypes.Structure):
    _fields_ = [("bmiHeader", BMIH), ("bmiColors", ctypes.c_uint32 * 3)]


class BLENDFUNC(ctypes.Structure):
    _fields_ = [("BlendOp", ctypes.c_ubyte), ("BlendFlags", ctypes.c_ubyte),
                ("SourceConstantAlpha", ctypes.c_ubyte), ("AlphaFormat", ctypes.c_ubyte)]


WNDPROC = ctypes.WINFUNCTYPE(ctypes.c_ssize_t, HWND, ctypes.c_uint,
                             ctypes.c_size_t, ctypes.c_ssize_t)


@WNDPROC
def _proc(hwnd, msg, wp, lp):
    return u.DefWindowProcW(hwnd, msg, wp, lp)


class WNDCLASSW(ctypes.Structure):
    _fields_ = [("style", ctypes.c_uint), ("lpfnWndProc", WNDPROC),
                ("cbClsExtra", ctypes.c_int), ("cbWndExtra", ctypes.c_int),
                ("hInstance", HINSTANCE), ("hIcon", HANDLE), ("hCursor", HANDLE),
                ("hbrBackground", HANDLE), ("lpszMenuName", w.LPCWSTR),
                ("lpszClassName", w.LPCWSTR)]


u.RegisterClassW.argtypes = [ctypes.POINTER(WNDCLASSW)]
u.CreateWindowExW.argtypes = [ctypes.c_uint, w.LPCWSTR, w.LPCWSTR, ctypes.c_uint,
                              ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                              HWND, HANDLE, HINSTANCE, ctypes.c_void_p]
u.CreateWindowExW.restype = HWND
u.UpdateLayeredWindow.argtypes = [HWND, HDC, ctypes.POINTER(w.POINT),
                                  ctypes.POINTER(w.SIZE), HDC, ctypes.POINTER(w.POINT),
                                  ctypes.c_uint32, ctypes.POINTER(BLENDFUNC),
                                  ctypes.c_uint32]
gdi.CreateDIBSection.argtypes = [HDC, ctypes.POINTER(BMI), ctypes.c_uint,
                                 ctypes.POINTER(ctypes.c_void_p), HANDLE, ctypes.c_uint32]
gdi.CreateDIBSection.restype = HANDLE


def make_intruder(tray, target_rect):
    """造一个**不透明品红**的分层兄弟窗，正好盖住 `target_rect`（屏幕坐标）。"""
    wc = WNDCLASSW()
    wc.lpfnWndProc = _proc
    wc.lpszClassName = INTRUDER
    # ⛔ 必须用 `GetStockObject(WHITE_BRUSH)` 的**返回值**，不能猜一个数字：
    #    「可见四条件」第 4 条要求类**有背景刷**（`NULL` ⇒ 分层窗整块不显示）。
    wc.hbrBackground = gdi.GetStockObject(0)  # WHITE_BRUSH
    u.RegisterClassW(ctypes.byref(wc))

    left, top, width, height = target_rect
    h = u.CreateWindowExW(
        WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
        INTRUDER, "zorder-intruder", WS_POPUP, 0, 0, width, height,
        None, None, None, None)
    if not h:
        return None
    # 与产品同序：先改样式，再 SetParent
    u.SetWindowLongPtrW(h, GWL_STYLE,
                        (u.GetWindowLongPtrW(h, GWL_STYLE) & ~WS_POPUP) | WS_CHILD)
    u.SetParent(h, tray)

    # 预乘 32bpp 品红（BGRA = 255,0,255,255）
    bmi = BMI()
    bmi.bmiHeader.biSize = ctypes.sizeof(BMIH)
    bmi.bmiHeader.biWidth = width
    bmi.bmiHeader.biHeight = -height          # 负 = top-down
    bmi.bmiHeader.biPlanes = 1
    bmi.bmiHeader.biBitCount = 32
    bmi.bmiHeader.biCompression = 0           # BI_RGB
    scr = u.GetDC(None)
    mem = gdi.CreateCompatibleDC(scr)
    bits = ctypes.c_void_p()
    hbmp = gdi.CreateDIBSection(mem, ctypes.byref(bmi), DIB_RGB_COLORS,
                                ctypes.byref(bits), None, 0)
    old = gdi.SelectObject(mem, hbmp)
    ctypes.memset(bits, 0, width * height * 4)
    buf = (ctypes.c_uint32 * (width * height)).from_address(bits.value)
    for i in range(width * height):
        buf[i] = 0xFFFF00FF                   # A=FF, R=FF, G=00, B=FF

    dst = w.POINT(left, top)
    u.ScreenToClient(tray, ctypes.byref(dst))
    size = w.SIZE(width, height)
    src = w.POINT(0, 0)
    blend = BLENDFUNC(0, 0, 255, 1)           # AC_SRC_OVER / AC_SRC_ALPHA
    ok = u.UpdateLayeredWindow(h, scr, ctypes.byref(dst), ctypes.byref(size),
                               mem, ctypes.byref(src), 0, ctypes.byref(blend), ULW_ALPHA)
    gdi.SelectObject(mem, old)
    gdi.DeleteObject(hbmp)
    gdi.DeleteDC(mem)
    u.ReleaseDC(None, scr)
    u.SetWindowPos(h, HWND_TOP, 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE)
    u.ShowWindow(h, 5)
    print(f"  入侵者 hwnd={h:#x} UpdateLayeredWindow={ok} dst(父客户区)=({dst.x},{dst.y})")
    return h


def wait_top(cls, timeout):
    """等 `cls` 回到第 0 位，返回耗时（秒）；超时返回 None。"""
    t0 = time.time()
    while time.time() - t0 < timeout:
        if zindex_of(cls) == 0:
            return time.time() - t0
        time.sleep(0.25)
    return None


# ══ 构建/还原 ═══════════════════════════════════════════════════

original = SRC.read_text(encoding="utf-8")
base_sha = hashlib.sha256(original.encode()).hexdigest()[:16]
print(f"源文件 sha256[:16] = {base_sha}")


def build(label):
    print(f"  构建中（{label}）…")
    r = subprocess.run([CARGO, "build"], cwd=str(SRC.parent.parent),
                       capture_output=True, text=True, encoding="utf-8", errors="replace")
    errs = [ln for ln in ((r.stdout or "") + (r.stderr or "")).splitlines()
            if ln.startswith("error") or "error[" in ln]
    if errs:
        print("⛔ 构建失败：\n" + "\n".join(errs[:15]))
        return False
    return True


def apply_inj(inj):
    txt = original
    desc, old, new = inj
    if txt.count(old) != 1:
        print(f"⛔ 注入锚点不唯一/未命中（{txt.count(old)} 处）：{desc}")
        return None
    txt = txt.replace(old, new, 1)
    SRC.write_text(txt, encoding="utf-8")
    print(f"  已注入：{desc}")
    return txt


def restore_src():
    SRC.write_text(original, encoding="utf-8")
    now = hashlib.sha256(SRC.read_text(encoding="utf-8").encode()).hexdigest()[:16]
    print(f"还原后 sha256[:16] = {now}  {'✅ 一致' if now == base_sha else '⛔ 不一致'}")


# ══ 主流程 ═════════════════════════════════════════════════════

kill()
shutil.copy2(CFG, CFG_BAK)
proc = None
intruder = None

try:
    # ────────────────────────── 阶段 1：干净构建 ──────────────────────────
    print("\n" + "=" * 62)
    print("阶段 1：干净构建 —— Z 序维护 + Explorer 重建路径的**对照基线**")
    print("=" * 62)
    if not build("干净"):
        sys.exit(2)

    proc, hwnd = start_app("干净构建")
    if not hwnd:
        check("P 前提：widget 已挂载", False, "30s 内未出现 widget")
        raise SystemExit(2)

    t = taskbar()
    wr = rect_of(hwnd)
    idx0 = zindex_of(OURS)
    print(f"  任务栏={t:#x}  widget rect={wr}  Z 序号={idx0}")
    check("P 前提：widget 已挂载且在最顶", idx0 == 0, f"Z 序号={idx0}")

    print("\n=== 造入侵者（同款形态 + 不透明品红，正好盖住 widget）===")
    intruder = make_intruder(t, wr)
    if not intruder:
        check("P 前提：入侵者已建起", False, "CreateWindowExW 失败")
        raise SystemExit(2)
    info_ratio = magenta_ratio(wr)
    print(f"  （信息）widget 区域品红占比 = {info_ratio:.2f}"
          f"  ⚠️ 仅信息：widget 多为 alpha=0 像素，下方品红会透出来，**不能**当判据")

    # ⭐ 以 0.1s 采样 8s：既记录「被压下去」是否被观测到，也记录恢复时刻。
    t0 = time.time()
    seq = []
    saw_down = False
    while time.time() - t0 < 8:
        wi = zindex_of(OURS)
        seq.append(wi)
        if wi not in (None, 0):
            saw_down = True
        time.sleep(0.1)
    idx_now = zindex_of(OURS)
    owner_now = owner_at(wr)
    log_lines = [ln.strip() for ln in log_text().splitlines() if "Z 序被后来者压住" in ln]
    nz = sum(1 for x in seq if x not in (None, 0))
    print(f"  widget Z 序号序列：{len(seq)} 个采样点，其中非 0 的有 {nz} 个")
    print(f"  8s 后：widget Z 序号={idx_now}  该点命中归属={owner_now}")
    for ln in log_lines:
        print("  LOG:", ln)
    # ⛔⛔ 前提：必须证明「入侵者确实把我们压下去过」，否则「恢复」可能只是
    #    「入侵者压根没上去」的假象。两个独立证据：
    #     ① 采样到过非 0 序号；② 应用自己打出「Z 序被后来者压住」——
    #        该日志**只在** `!is_top_sibling` 时写，不可能凭空出现。
    check("P 前提：入侵者确实压住过 widget", saw_down or len(log_lines) >= 1,
          f"采样到非0={saw_down}；应用日志 {len(log_lines)} 条")
    check("A 维护把 widget 拉回兄弟 Z 序第 0 位（≤8s）", idx_now == 0, f"Z 序号={idx_now}")
    check("B 恢复后该点命中归属回到 widget（不再被入侵者接管）",
          owner_now == OURS, f"归属={owner_now}")
    check("G 日志留下「Z 序被后来者压住」", len(log_lines) >= 1, f"{len(log_lines)} 条")

    print("\n=== D-基线. 无修复时：widget 被销毁后 10s 内不恢复（30s 慢节拍）===")
    print("  （用 WM_CLOSE 让窗口在主线程自行销毁，等价于「被连带销毁」）")
    u.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    time.sleep(1.5)
    gone = widget_hwnd()
    check("P 前提：WM_CLOSE 确实销毁了 widget", not gone or gone == 0,
          f"hwnd={gone}")
    t0 = time.time()
    back = None
    while time.time() - t0 < 10:
        back = widget_hwnd()
        if back:
            break
        time.sleep(0.25)
    check("D-基线 10s 内**不**恢复（证明慢节拍确实是瓶颈）", not back,
          f"实际 {time.time() - t0:.1f}s 就回来了" if back else "10s 内未回来 ✅")

    proc.terminate(); time.sleep(1); kill()
    if intruder:
        u.DestroyWindow(intruder); intruder = None

    # ────────────────────── 阶段 2：注入「禁用维护重申」 ──────────────────────
    print("\n" + "=" * 62)
    print("阶段 2：可证伪 —— 注入「禁用维护期的 Z 序重申」⇒ A 必须转红")
    print("=" * 62)
    if apply_inj(INJ_RAISE) is None or not build("注入：禁用重申"):
        raise SystemExit(2)

    proc, hwnd = start_app("注入：禁用维护重申")
    if not hwnd:
        check("C 注入后仍能挂载", False, "未出现 widget")
        raise SystemExit(2)
    wr = rect_of(hwnd)
    check("P 前提（阶段2）：widget 初始在最顶", zindex_of(OURS) == 0,
          f"Z 序号={zindex_of(OURS)}")
    intruder = make_intruder(taskbar(), wr)
    time.sleep(0.6)
    i_idx, w_idx = zindex_of(INTRUDER), zindex_of(OURS)
    check("P 前提（阶段2）：入侵者确实压在 widget 之上",
          i_idx is not None and w_idx is not None and i_idx < w_idx,
          f"入侵者={i_idx} < widget={w_idx}")
    took = wait_top(OURS, 8)
    check("C 注入后 **8s 内无法恢复**（证明 A 的判据可证伪）", took is None,
          f"竟然 {took:.1f}s 就恢复了 ⇒ A 的判据失效" if took is not None else "8s 内未恢复 ✅")
    owner_c = owner_at(wr)
    check("C 注入后 widget 区域的命中归属仍是入侵者", owner_c == INTRUDER,
          f"归属={owner_c}")

    proc.terminate(); time.sleep(1); kill()
    if intruder:
        u.DestroyWindow(intruder); intruder = None

    # ─────────────────── 阶段 3：注入「任务栏句柄恒变」验证快路径 ───────────────────
    print("\n" + "=" * 62)
    print("阶段 3：Explorer 重建快路径 —— 注入「句柄恒变」⇒ 销毁后 ≤5s 恢复")
    print("=" * 62)
    if apply_inj(INJ_CHANGED) is None or not build("注入：句柄恒变"):
        raise SystemExit(2)

    proc, hwnd = start_app("注入：句柄恒变", extra_env={"PM_TEST_FORCE_REBUILD": "1"})
    if not hwnd:
        check("D 注入后仍能挂载", False, "未出现 widget")
        raise SystemExit(2)
    u.PostMessageW(hwnd, WM_CLOSE, 0, 0)
    time.sleep(1.5)
    check("P 前提（阶段3）：widget 已销毁", not widget_hwnd(), "")
    t0 = time.time()
    back = None
    while time.time() - t0 < 6:
        back = widget_hwnd()
        if back:
            break
        time.sleep(0.25)
    dt = time.time() - t0
    check("D 修复后 ≤5s 恢复（对比基线：10s 内不恢复）", back and dt <= 5.0,
          f"{dt:.1f}s 恢复，hwnd={back}" if back else "6s 内未恢复")
    if back:
        time.sleep(1.5)
        check("D 恢复后的 widget 也在最顶", zindex_of(OURS) == 0,
              f"Z 序号={zindex_of(OURS)}")

finally:
    print("\n=== 收尾 ===")
    if proc:
        proc.terminate()
    time.sleep(1)
    kill()
    if intruder:
        try:
            u.DestroyWindow(intruder)
        except Exception:
            pass
    shutil.copy2(CFG_BAK, CFG)
    print("config 已还原")
    restore_src()
    build("恢复正式产物")

bad = [n for n, ok, _ in RESULTS if not ok]
print(f"\n{'=' * 60}")
print(f"结果：{len(RESULTS) - len(bad)}/{len(RESULTS)} 通过")
if bad:
    print("失败项：" + "、".join(bad))
    sys.exit(1)
print("✅ 全部通过")

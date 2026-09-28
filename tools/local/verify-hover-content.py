# -*- coding: utf-8 -*-
"""验收（问题 1）：hover 时**内容不能被侵蚀变细**。

背景：底衬把整块区域写成 alpha=153，而旧的合成写法是「取最大 alpha」
（`if a > dst_a { dst = packed }`）⇒ 抗锯齿边缘（覆盖度 < 153）被丢弃 ⇒ 字形笔画变细。
修法 = 标准 source-over（`blend_over`）。

## 方法：**隐藏 widget 的差分**（不是「局部背景 + 绝对亮度」）

`内容掩膜 = 显示态比「widget 隐藏时」明显更暗的像素`，再按**覆盖度**归一：
`c = 1 − shown / (bg + L)`（`L` = 底衬抬升，从永不落内容的左侧内边距列量出）。
`c > 0.15` 记「有墨」，`c > 0.6` 记「实心墨」。

⛔⛔ 为什么必须差分（两次踩坑换来的）：
   · 任务栏**不是均匀底**：左侧有搜索框高亮区（+22），右端有**系统托盘**（图标 + 时钟）。
   · 首版用固定阈值 ⇒ 撞搜索框；改用「同行窗口外」参考 ⇒ 参考点离得太远，撞背景差。
   · 改用「局部背景（±7px 最大值）+ 相对覆盖度」后 **D 判据仍报 21.9%** ——
     排查到底：自动摆位把窗口放到了最右端 x=2206，**正好压住系统托盘**；
     逐像素比对证实那些「暗像素」与「widget 不在那里」时**逐字节相同**（`(25,26,19)`）
     ⇒ 是**托盘的时钟/图标**，不是我们的内容。差分法从根上消掉这一类干扰：
     `bg` 里已经含了托盘/应用按钮/搜索框，相减即净。

⛔⛔ **态 A 必须真的是非 hover**（否则 D 变成「底衬态 vs 底衬态」，旧写法同样通过 ＝ 假绿）：
   新增 **A0 前置判据**（`L < 5`），不成立就**直接中止**，不输出可能为假的 D。
   自动摆位最多试 3 次，仍失败则提示用户把鼠标移开任务栏。

## 判据
  A0 态 A 确实非 hover（无底衬） · B hover 成立（光标在窗内） · C hover 时底衬确实出现
  D 两态内容量同量级（差 < 25%） · D2 判据**有区分力**（软边像素足够多，旧写法才会转红）
  E hover 态仍有纯黑实心笔画
"""

import ctypes
import ctypes.wintypes as wt
import os
import re
import struct
import subprocess
import sys
import time
import zlib

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.hoverbak")
OUT_DIR = r"D:\Code\PeriTray\tools\local\_out"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

WIDGET_CLASS = "PeriTrayTaskbarWidget"
WAIT_CURSOR_S = 240
WAIT_LEAVE_S = 120
# 覆盖度阈值：软边（含抗锯齿）/ 实心墨。见文件头「方法」。
COV_SOFT = 0.15
COV_INK = 0.60

u = ctypes.WinDLL("user32", use_last_error=True)
gdi = ctypes.WinDLL("gdi32", use_last_error=True)
u.SetProcessDpiAwarenessContext.argtypes = [ctypes.c_void_p]
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))    # 本机 125%
u.FindWindowW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p]
u.FindWindowW.restype = ctypes.c_void_p
u.FindWindowExW.argtypes = [ctypes.c_void_p, ctypes.c_void_p, ctypes.c_wchar_p, ctypes.c_wchar_p]
u.FindWindowExW.restype = ctypes.c_void_p
u.GetWindowRect.argtypes = [ctypes.c_void_p, ctypes.POINTER(wt.RECT)]
u.GetWindowRect.restype = wt.BOOL
u.GetCursorPos.argtypes = [ctypes.POINTER(wt.POINT)]
u.GetCursorPos.restype = wt.BOOL
u.ClientToScreen.argtypes = [ctypes.c_void_p, ctypes.POINTER(wt.POINT)]
u.GetParent.argtypes = [ctypes.c_void_p]
u.GetParent.restype = ctypes.c_void_p
u.GetDC.argtypes = [ctypes.c_void_p]
u.GetDC.restype = ctypes.c_void_p
u.ReleaseDC.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
gdi.CreateCompatibleDC.argtypes = [ctypes.c_void_p]
gdi.CreateCompatibleDC.restype = ctypes.c_void_p
gdi.CreateCompatibleBitmap.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int]
gdi.CreateCompatibleBitmap.restype = ctypes.c_void_p
gdi.SelectObject.argtypes = [ctypes.c_void_p, ctypes.c_void_p]
gdi.SelectObject.restype = ctypes.c_void_p
gdi.DeleteObject.argtypes = [ctypes.c_void_p]
gdi.DeleteDC.argtypes = [ctypes.c_void_p]
gdi.BitBlt.argtypes = [ctypes.c_void_p, ctypes.c_int, ctypes.c_int, ctypes.c_int, ctypes.c_int,
                       ctypes.c_void_p, ctypes.c_int, ctypes.c_int, wt.DWORD]
gdi.GetDIBits.argtypes = [ctypes.c_void_p, ctypes.c_void_p, wt.UINT, wt.UINT, ctypes.c_void_p,
                          ctypes.c_void_p, wt.UINT]


class BIH(ctypes.Structure):
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


def grab(left, top, w, h):
    """抓屏 → top-down BGRA bytes。"""
    hdc = u.GetDC(None)
    mem = gdi.CreateCompatibleDC(hdc)
    bmp = gdi.CreateCompatibleBitmap(hdc, w, h)
    old = gdi.SelectObject(mem, bmp)
    gdi.BitBlt(mem, 0, 0, w, h, hdc, left, top, 0x40000000 | 0x00CC0020)   # CAPTUREBLT|SRCCOPY
    bih = BIH()
    bih.biSize = ctypes.sizeof(BIH)
    bih.biWidth = w
    bih.biHeight = -h
    bih.biPlanes = 1
    bih.biBitCount = 32
    buf = ctypes.create_string_buffer(w * h * 4)
    gdi.GetDIBits(mem, bmp, 0, h, buf, ctypes.byref(bih), 0)
    gdi.SelectObject(mem, old)
    gdi.DeleteObject(bmp)
    gdi.DeleteDC(mem)
    u.ReleaseDC(None, hdc)
    return buf.raw        # ⛔ 必须 .raw


def rgb_at(d, w, x, y):
    i = (y * w + x) * 4
    return d[i + 2], d[i + 1], d[i]


def lum(rgb):
    return 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]


def lum_map(data, w, h):
    return [[lum(rgb_at(data, w, x, y)) for x in range(w)] for y in range(h)]


# ── 极简 PNG 写入（stdlib zlib，无第三方依赖）──────────────────────
def write_png(path, data, w, h):
    raw = bytearray()
    for y in range(h):
        raw.append(0)                                    # filter = None
        for x in range(w):
            r, g, b = rgb_at(data, w, x, y)
            raw += bytes((r, g, b))

    def chunk(tag, payload):
        return (struct.pack(">I", len(payload)) + tag + payload
                + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF))

    png = b"\x89PNG\r\n\x1a\n"
    png += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 2, 0, 0, 0))
    png += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    png += chunk(b"IEND", b"")
    with open(path, "wb") as f:
        f.write(png)


# ── 差分统计（见文件头「方法」）──────────────────────────────────
def diff_stats(shown, bg, w, h):
    """返回 `(n_soft, n_ink, lift, darkest)`。

    ⭐ `lift` = 底衬抬升，取**左内边距列**（local x=2）的中位差 —— 该列永不落内容，
       所以「显示态 − 背景态」就是纯底衬亮度；非 hover 时应 ≈ 0。
    ⭐ `c = 1 − shown/(bg + lift)` 是**覆盖度**，与脚下背景亮度无关 ⇒ 两态可比。
       ⛔ 不能直接用 `bg − shown` 当判据：底衬把背景抬亮后，同一覆盖度的边缘像素
       差值会缩水（实测 c=0.2 时 47 → 33），两态阈值不等价 ⇒ 白送假红。
    """
    Ls = lum_map(shown, w, h)
    Lb = lum_map(bg, w, h)
    y0, y1 = 12, max(13, h - 12)          # 避开圆角（radius=8）边缘
    col = sorted(Ls[y][2] - Lb[y][2] for y in range(y0, y1))
    lift = col[len(col) // 2]
    n_soft = n_ink = 0
    darkest = 999.0
    for y in range(h):
        for x in range(w):
            denom = Lb[y][x] + lift
            if denom <= 1.0:
                continue
            c = 1.0 - Ls[y][x] / denom
            if c > COV_SOFT:
                n_soft += 1
                darkest = min(darkest, Ls[y][x])
                if c > COV_INK:
                    n_ink += 1
    return n_soft, n_ink, lift, darkest


def measure(rect, shown, bg):
    n_soft, n_ink, lift, darkest = diff_stats(shown, bg, rect[2], rect[3])
    return {"soft": n_soft, "ink": n_ink, "lift": lift, "darkest": darkest,
            "edge": n_soft - n_ink}


# ── 配置 ───────────────────────────────────────────────────────────
def drop_key(key):
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    open(CFG, "w", encoding="utf-8").write(
        re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M))


def set_key(key, line):
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    txt = re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M)
    open(CFG, "w", encoding="utf-8").write(line + "\n" + txt)


def drop_pins():
    """清空 pin 列表（TOML **数组表**，不是标量键）。

    ⛔⛔ `pinned_taskbar_devices` 在 config.toml 里是 `[[pinned_taskbar_devices]]` **数组表**，
       而 `drop_key()` 用的是 `^key\\s*=` 这种**标量键**写法 ⇒ **匹配不到、静默无效**
       （实测：清了 key，pin 一个没少，widget 照旧显示 ⇒ 差分法的「背景态」抓到的还是
       带 widget 的画面 ⇒ 内容像素数 ≈ 0，四条判据全假红）。
       ⇒ 必须按 TOML 语义「表头切换上下文」来删：从 `[[pinned_taskbar_devices]]` 起，
       到下一个 `[`/`[[` 表头之前的**全部行**（字段 / 注释 / 空行）都丢掉。
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
       "  for (const d of pick) {"
       "    if (pinned.has(d.key)) continue;"
       "    await inv('toggle_pinned_taskbar_device', { key: d.key, fallback: null, alias: null });"
       "  }"
       "  return JSON.stringify(pick.map(d => d.name)); })()")


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


def launch():
    env = dict(os.environ)
    env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    return subprocess.Popen([EXE], cwd=WD, env=env)


def start_app(pin=False, want_widget=True, wait=9):
    """启动应用；`want_widget=False` 时不等待 widget（用于「隐藏态」背景抓取）。"""
    proc = launch()
    time.sleep(wait)
    hwnd = 0
    for _ in range(3):
        if pin:
            print("  pinned:", cdp(PIN))
        for _ in range(20):
            hwnd = widget()
            if hwnd:
                break
            time.sleep(0.5)
        if hwnd or not want_widget:
            break
    time.sleep(1.5)
    return proc, hwnd


def taskbar():
    return u.FindWindowW("Shell_TrayWnd", None)


def widget():
    t = taskbar()
    return u.FindWindowExW(t, None, WIDGET_CLASS, None) if t else 0


def rect_of(hwnd):
    r = wt.RECT()
    if not hwnd or not u.GetWindowRect(hwnd, ctypes.byref(r)):
        return None
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def cursor():
    p = wt.POINT()
    return (p.x, p.y) if u.GetCursorPos(ctypes.byref(p)) else None


def cursor_in(rect):
    pt, rc = cursor(), rect
    return bool(pt and rc and rc[0] <= pt[0] < rc[0] + rc[2] and rc[1] <= pt[1] < rc[1] + rc[3])


def client_left(hwnd):
    p = wt.POINT(0, 0)
    u.ClientToScreen(u.GetParent(hwnd), ctypes.byref(p))
    return p.x


def far_from_cursor(tb, cw):
    """`taskbar_custom_x` 的**相对**取值（相对任务栏客户区左边缘），取离光标最远的一端。"""
    pt = cursor() or (tb[0], tb[1])
    hi = max(0, tb[2] - cw)
    return 0 if pt[0] > tb[0] + tb[2] // 2 else hi


def hide_and_grab(rect):
    """清空 pin ⇒ widget 隐藏 ⇒ 抓同一块作**背景**（差分法的对照）。

    ⛔ 顺序必须是 **先 kill 再改配置**：应用运行期间改文件，退出时可能被内存态写回覆盖。
    ⛔ 抓之前**必须确认 widget 真的没了** —— 否则「背景态」抓到的还是带 widget 的画面，
      差分恒为 0 ⇒ 判据全假红（本脚本就踩过一次，根因见 `drop_pins` 的文档）。
    """
    kill()
    drop_pins()
    start_app(want_widget=False)
    if widget():
        raise RuntimeError("清 pin 后 widget 仍在 ⇒ 背景态无效（检查 config 的 pin 存储形态）")
    return grab(rect[0], rect[1], rect[2], rect[3])


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml，先手动跑一次应用")
        return 1
    open(CFG_BAK, "w", encoding="utf-8").write(open(CFG, encoding="utf-8").read())
    proc = None
    try:
        print("=== 准备 ===")
        kill()
        drop_pins()
        drop_key("taskbar_custom_x")
        set_key("taskbar_position_locked", "taskbar_position_locked = false")
        proc, hwnd = start_app(pin=True)
        if not hwnd:
            print("!! widget 未出现")
            return 1
        tb = rect_of(taskbar())
        rect = rect_of(hwnd)
        note(f"任务栏 = {tb}")
        note(f"widget = {rect}（高 {rect[3]}）")

        # ── 态 A：非 hover ────────────────────────────────────────
        print("\n=== 态 A：非 hover ===")
        for attempt in range(3):
            kill()
            set_key("taskbar_custom_x", f"taskbar_custom_x = {far_from_cursor(tb, rect[2])}")
            proc, hwnd = start_app(pin=True)
            rect = rect_of(hwnd)
            if not cursor_in(rect):
                break
            note(f"试 {attempt + 1}：widget = {rect}（光标仍在窗内 ⇒ 换另一端）")
        if cursor_in(rect):
            print(f"  ⚠️ 自动摆位后光标仍在窗内 ⇒ 请把鼠标**移开任务栏**，最多等 {WAIT_LEAVE_S}s …")
            t0 = time.time()
            while time.time() - t0 < WAIT_LEAVE_S:
                time.sleep(1.0)
                if not cursor_in(rect):
                    break
        note(f"widget = {rect}（光标在内 = {cursor_in(rect)}）")
        shown_off = grab(rect[0], rect[1], rect[2], rect[3])
        bg_off = hide_and_grab(rect)
        off = measure(rect, shown_off, bg_off)
        write_png(os.path.join(OUT_DIR, "hover-content-off.png"), shown_off, rect[2], rect[3])
        a0 = off["lift"] < 5.0
        check("A0 态 A 确实**非** hover（底衬未出现）", a0,
              f"底衬抬升 {off['lift']:+.1f}（须 < 5）"
              + ("" if not cursor_in(rect) else "；⚠️ 光标仍在窗内"))
        if not a0:
            print("!! 无法建立非 hover 对照态 ⇒ 此时 D 会是假绿（两态都带底衬），已中止")
            return 2
        note(f"内容像素 {off['soft']}（实心 {off['ink']}，软边 {off['edge']}）"
             f"/ 最深 {off['darkest']:.1f}")

        # ── 态 B：hover ──────────────────────────────────────────
        print(f"\n=== 态 B：hover（最多等 {WAIT_CURSOR_S}s）===")
        hovered = False
        t0 = time.time()
        while time.time() - t0 < WAIT_CURSOR_S:
            pt = cursor()
            if pt and tb[0] <= pt[0] < tb[0] + tb[2] and tb[1] <= pt[1] < tb[1] + tb[3]:
                cw = rect[2]
                tgt = max(0, min(pt[0] - cw // 2 - client_left(hwnd), tb[2] - cw))
                kill()
                set_key("taskbar_custom_x", f"taskbar_custom_x = {tgt}")
                proc, hwnd = start_app(pin=True)
                rect = rect_of(hwnd)
                if cursor_in(rect):
                    hovered = True
                    break
            else:
                time.sleep(0.5)
        check("B hover 成立（光标落在窗口内）", hovered, f"widget={rect}")
        if not hovered:
            return 2
        time.sleep(1.0)
        shown_on = grab(rect[0], rect[1], rect[2], rect[3])
        bg_on = hide_and_grab(rect)
        on = measure(rect, shown_on, bg_on)
        write_png(os.path.join(OUT_DIR, "hover-content-on.png"), shown_on, rect[2], rect[3])
        note(f"内容像素 {on['soft']}（实心 {on['ink']}，软边 {on['edge']}）"
             f"/ 最深 {on['darkest']:.1f}")

        # ── 判据 ──────────────────────────────────────────────────
        print("\n=== 判据 ===")
        check("C hover 时底衬确实出现（背景被抬亮）", on["lift"] > 10.0,
              f"底衬抬升 {on['lift']:+.1f}（须 > 10）")
        # ⛔ 旧写法（取最大 alpha）下，覆盖度 ≤ 0.6 的边缘像素被底衬顶掉 ⇒ 内容量会**明显掉**
        lo, hi = min(off["soft"], on["soft"]), max(off["soft"], on["soft"])
        ratio = (hi - lo) / hi if hi else 1.0
        check("D hover 态内容像素数与非 hover 态**同量级**（内容未被侵蚀变细）",
              ratio < 0.25,
              f"非 hover {off['soft']} vs hover {on['soft']}（相差 {ratio * 100:.1f}%，阈值 25%）")
        # ⭐ D2 = D 的**区分力证明**：旧写法丢掉的是「软边」那批（覆盖度 ≤ 0.6）。
        #    ⛔ 若软边数太少，D 即使通过也说明不了问题（判据对侵蚀不敏感）＝假绿。
        check("D2 判据有区分力（软边像素足够多，旧写法必转红）",
              off["edge"] > 300 and on["edge"] > 300,
              f"非 hover 软边 {off['edge']} vs hover {on['edge']}（均须 > 300）")
        check("E hover 态仍存在**纯黑**实心笔画（内容确实被画上去）", on["darkest"] < 60,
              f"最深 {on['darkest']:.1f}")

        print("\n=== 结果 ===")
        ok = sum(1 for _, o, _ in RESULTS if o)
        for n, o, d in RESULTS:
            if not o:
                print(f"  ❌ {n}  —— {d}")
        print(f"{ok}/{len(RESULTS)} 通过")
        print(f"图像：{OUT_DIR}\\hover-content-off.png / hover-content-on.png")
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

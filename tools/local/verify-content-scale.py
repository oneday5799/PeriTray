# -*- coding: utf-8 -*-
"""验收：「任务栏内容缩放大小」只改**内容**，底衬恒定。

## 被测口径（用户 2026-09-25 指定）
  · `default`（默认档）：内容按 96 DPI ⇒ 125% 系统缩放下图标 32 / 字号 11；
  · `follow_system`：内容与底衬同按系统 DPI ⇒ 图标 40 / 字号 14；
  · ⛔ **底衬两档必须都 = 50px 高**（40 DIP × 1.25）—— 这是本设置的**硬红线**。

## 方法
  · **同一次运行内切档**（CDP 调 `update_config`）—— 两次测量的**设备集合完全相同**，
    宽度才可比。若像上一版那样「每档重启一次」，CDP 的 pin 选择可能落在不同设备上
    （实测拿到 4 台 vs 2 台）⇒ 宽度差异来自数据而不是档位，判据直接失去区分力。
    ⭐ 这同时顺带验证了「改档**无需重启**即生效」。
  · **内容高度**：**非 hover** 态相对「widget 隐藏」画面的覆盖度掩膜，取有墨行的 y 跨度。
    ⛔ 必须用非 hover 态：hover 底衬的**圆角边缘**是部分透明像素，会被同一掩膜判成「墨」
       ⇒ bbox 被撑到窗口高度（50），测出来的是底衬而不是内容。
  · **底衬高度**：hover 态相对**同档位的 plain 态**，x=2 列（左内边距，永不落内容）
    的亮度抬升 > 3 的行数。
    ⛔⛔ **阈值必须用「hover − plain」，不能用「hover − 无widget背景」**：
       本机任务栏那块底色很亮（≈230），白色 alpha=153 叠上去只抬升 ≈15
       ⇒ 上一版按「深色背景 ≈ +138」定的 60 阈值**必然假红**（实测 14.2 被判失败，
       而抓图里底衬明明就在）。**同一档位内相减**与背景亮度无关。

## 判据
  A0 非 hover 态确实没有底衬（内容测量有效）—— 不成立**直接中止**
  A1 hover 态底衬确实出现（底衬测量有效）—— 不成立**直接中止**
  B  两档的内容掩膜都有效（有墨行数 > 10）
  C  `follow_system` 的内容明显**更高**（档位真的改了内容）
  D  比例落在 [0.7, 0.9]（预期 32/40 = 0.8）
  E  两档底衬高度**都是 50**（±2）—— 底衬不随档位改变
  F  区分力：内容高度必须 **< 底衬高度**（default 档 32 < 50）
  G  窗口宽度随内容变宽（设备集合固定 ⇒ 差异只可能来自档位）
"""

import ctypes
import ctypes.wintypes as wt
import json
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
CFG_BAK = os.path.join(WD, "config.toml.scalebak")
OUT_DIR = r"D:\Code\PeriTray\tools\local\_out"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

WIDGET_CLASS = "PeriTrayTaskbarWidget"
COV_SOFT = 0.15          # 覆盖度：软边（含抗锯齿）
LIFT_HOVER = 3.0         # 底衬抬升（hover − plain，与背景亮度无关）
LIFT_PLAIN_MAX = 5.0     # 非 hover 判据（plain − bg）
EXPECT_BACKDROP_H = 50   # 40 DIP × 1.25
CURSOR_AWAY = (1280, 400)   # 远离任务栏（窗口只占任务栏那 50px 带）

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
u.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u.SetCursorPos.restype = wt.BOOL
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


# ── 抓屏 ──────────────────────────────────────────────────────────
def grab(left, top, w, h):
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
    return buf.raw


def rgb_at(d, w, x, y):
    i = (y * w + x) * 4
    return d[i + 2], d[i + 1], d[i]


def lum(rgb):
    return 0.299 * rgb[0] + 0.587 * rgb[1] + 0.114 * rgb[2]


def lum_map(data, w, h):
    return [[lum(rgb_at(data, w, x, y)) for x in range(w)] for y in range(h)]


def write_png(path, data, w, h):
    raw = bytearray()
    for y in range(h):
        raw.append(0)
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


def crop(full, fw, fh, x, y, w, h):
    """从整幅 BGRA 里裁一块（供「整条任务栏」背景复用给不同位置的窗口）。"""
    out = bytearray()
    for yy in range(y, y + h):
        row = yy * fw * 4
        out += full[row + x * 4: row + (x + w) * 4]
    return bytes(out)


# ── 两个量 ────────────────────────────────────────────────────────
def lift_at(a, b, w, h):
    """x=2 列（左内边距，永不落内容）的**中位**亮度差 `a − b` = 底衬抬升。"""
    La, Lb = lum_map(a, w, h), lum_map(b, w, h)
    y0, y1 = 12, max(13, h - 12)          # 避开圆角（radius=8）
    col = sorted(La[y][2] - Lb[y][2] for y in range(y0, y1))
    return col[len(col) // 2]


def content_rows(shown, bg, w, h, lift):
    """有墨（覆盖度 > COV_SOFT）的行号列表。**非 hover 态**下 lift ≈ 0。"""
    Ls, Lb = lum_map(shown, w, h), lum_map(bg, w, h)
    rows = []
    for y in range(h):
        for x in range(w):
            denom = Lb[y][x] + lift
            if denom <= 1.0:
                continue
            if 1.0 - Ls[y][x] / denom > COV_SOFT:
                rows.append(y)
                break
    return rows


def backdrop_rows(hover, plain, w, h):
    """底衬覆盖的行号（行内**中位**抬升 > 阈值：hover 比 plain 亮）。

    ⛔ 不能用**单列**（如 x=2）：那一列在左圆角带内（x < radius=8），
      圆角把 y<3 与 y>46 挖掉 ⇒ 实测恒 44 行而不是 50 —— 这是**几何结果不是缺陷**
      （`inside_rounded_rect(2, y, 50, 8)` 解出 y ∈ [3, 46]）。
      取行内中位：y=0/49 行的**中间列**仍有完整底衬 ⇒ 高度回到 50。
    """
    Lh, Lp = lum_map(hover, w, h), lum_map(plain, w, h)
    rows = []
    for y in range(h):
        vals = sorted(Lh[y][x] - Lp[y][x] for x in range(w))
        if vals[len(vals) // 2] > LIFT_HOVER:
            rows.append(y)
    return rows


# ── 配置 / 进程 ───────────────────────────────────────────────────
def drop_pins():
    """清空 pin 列表（TOML **数组表** `[[pinned_taskbar_devices]]`，不是标量键）。

    ⛔ `^key\\s*=` 这种标量键写法**匹配不到数组表** ⇒ 静默无效 ⇒
       「背景态」抓到的还是带 widget 的画面 ⇒ 内容像素 ≈ 0 ⇒ 判据全假红。
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


def set_scale(value):
    """**不改文件、不重启**：走 CDP 调 `update_config`（base 与 patch 只差该字段）。

    ⭐ 这既是测量手段，也是被测行为本身 —— 若切档不生效，后续 rect/内容立刻暴露。
    """
    expr = ("(async () => {"
            "  const inv = window.__TAURI__.core.invoke;"
            "  const before = await inv('get_config');"
            "  const after = JSON.parse(JSON.stringify(before));"
            f"  after.taskbar_content_scale = '{value}';"
            "  await inv('update_config', { base: before, newConfig: after });"
            "  return JSON.stringify(after.taskbar_content_scale); })()")
    return cdp(expr)


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


def launch():
    env = dict(os.environ)
    env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    return subprocess.Popen([EXE], cwd=WD, env=env)


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


def set_cursor(x, y, settle=0.6):
    u.SetCursorPos(int(x), int(y))
    time.sleep(settle)


def cursor():
    p = wt.POINT()
    return (p.x, p.y) if u.GetCursorPos(ctypes.byref(p)) else None


def wait_stable_rect(timeout=12):
    """等窗口宽度稳定（改档后要重算避让槽 + 重绘，且首帧可能是旧数据）。"""
    last, same, t0 = None, 0, time.time()
    while time.time() - t0 < timeout:
        h = widget()
        r = rect_of(h) if h else None
        if r and r == last:
            same += 1
            if same >= 3:
                return r
        else:
            same = 0
        last = r
        time.sleep(0.7)
    return last


def grab_hover_until(rect, min_lift=3.0, timeout=8.0):
    """把光标移到窗口中心，轮询直到底衬出现（返回 (hover图, plain图, lift)）。"""
    set_cursor(*CURSOR_AWAY)
    plain = grab(rect[0], rect[1], rect[2], rect[3])
    set_cursor(rect[0] + rect[2] // 2, rect[1] + rect[3] // 2, settle=0.0)
    t0, hover, lift = time.time(), None, 0.0
    while time.time() - t0 < timeout:
        time.sleep(0.45)
        hover = grab(rect[0], rect[1], rect[2], rect[3])
        lift = lift_at(hover, plain, rect[2], rect[3])
        if lift > min_lift:
            break
    return hover, plain, lift


def measure_scale(label, value, bg_full, tb):
    """切档 → 等稳定 → 抓 plain / hover → 返回各量。"""
    set_scale(value)
    rect = wait_stable_rect()
    if not rect:
        raise RuntimeError(f"[{label}] 切档后 widget 消失")
    note(f"[{label}] 档位={value} widget={rect}")
    hover, plain, lift_hover = grab_hover_until(rect)
    x_in_bg, y_in_bg = rect[0] - tb[0], rect[1] - tb[1]
    bg = crop(bg_full, tb[2], tb[3], x_in_bg, y_in_bg, rect[2], rect[3])
    lift_plain = lift_at(plain, bg, rect[2], rect[3])
    rows = content_rows(plain, bg, rect[2], rect[3], lift_plain)
    bd = backdrop_rows(hover, plain, rect[2], rect[3])
    for tag, img in (("plain", plain), ("hover", hover), ("bg", bg)):
        write_png(os.path.join(OUT_DIR, f"_scale_{label}_{tag}.png"), img, rect[2], rect[3])
    note(f"[{label}] lift(plain−bg)={lift_plain:.1f} lift(hover−plain)={lift_hover:.1f} "
         f"内容行={len(rows)} 底衬行={len(bd)}")
    return {
        "lift_plain": lift_plain,
        "lift_hover": lift_hover,
        "content_h": (max(rows) - min(rows) + 1) if rows else 0,
        "backdrop_h": (max(bd) - min(bd) + 1) if bd else 0,
        "w": rect[2],
        "h": rect[3],
    }


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml，先手动跑一次应用")
        return 1
    open(CFG_BAK, "w", encoding="utf-8").write(open(CFG, encoding="utf-8").read())
    try:
        # ── 背景态：清 pin ⇒ 无 widget ⇒ 抓整条任务栏带 ──
        print("=== 背景态（无 widget 的整条任务栏）===")
        kill()
        drop_pins()
        launch()
        time.sleep(9)
        if widget():
            raise RuntimeError("清 pin 后 widget 仍在 ⇒ 背景态无效")
        tb = rect_of(taskbar())
        set_cursor(*CURSOR_AWAY, settle=1.2)     # 避免光标停在任务栏按钮上产生高亮
        bg_full = grab(tb[0], tb[1], tb[2], tb[3])
        write_png(os.path.join(OUT_DIR, "_scale_tb_bg.png"), bg_full, tb[2], tb[3])
        note(f"taskbar = {tb}")

        # ── 同一个进程内 pin 设备，再切两次档 ──
        print("=== pin 设备（同一次运行内）===")
        print("  pinned:", cdp(PIN))
        for _ in range(20):
            if widget():
                break
            time.sleep(0.5)
        if not widget():
            raise RuntimeError("pin 后 widget 未出现")

        print("=== 档位 A：default（不跟随系统缩放）===")
        a = measure_scale("default", "default", bg_full, tb)
        print("=== 档位 B：follow_system（跟随系统缩放）===")
        b = measure_scale("follow", "follow_system", bg_full, tb)

        print("=== 判据 ===")
        check("A0 非 hover 态确实无底衬（内容测量有效）",
              a["lift_plain"] < LIFT_PLAIN_MAX and b["lift_plain"] < LIFT_PLAIN_MAX,
              f"lift(plain−bg) = {a['lift_plain']:.1f} / {b['lift_plain']:.1f}")
        check("A1 hover 态底衬确实出现（底衬测量有效）",
              a["lift_hover"] > LIFT_HOVER and b["lift_hover"] > LIFT_HOVER,
              f"lift(hover−plain) = {a['lift_hover']:.1f} / {b['lift_hover']:.1f}")

        ca, cb = a["content_h"], b["content_h"]
        check("B 两档内容掩膜都有效（行数 > 10）", ca > 10 and cb > 10, f"内容高 = {ca} / {cb}")
        check("C follow_system 的内容明显更高（档位真的改了内容）", cb > ca, f"{ca} → {cb}")
        ratio = (ca / cb) if cb else 0.0
        check("D 比例落在 [0.7, 0.9]（预期 32/40 = 0.8）", 0.70 <= ratio <= 0.90,
              f"ratio = {ratio:.3f}（{ca}/{cb}）")
        check("E 两档底衬高度都是 50（±2）",
              abs(a["backdrop_h"] - EXPECT_BACKDROP_H) <= 2
              and abs(b["backdrop_h"] - EXPECT_BACKDROP_H) <= 2,
              f"底衬高 = {a['backdrop_h']} / {b['backdrop_h']}（窗口高 {a['h']}/{b['h']}）")
        check("F 区分力：内容高 < 底衬高（否则测的是底衬不是内容）",
              ca < a["backdrop_h"] and cb <= b["backdrop_h"],
              f"内容 {ca}/{cb} vs 底衬 {a['backdrop_h']}/{b['backdrop_h']}")
        check("G 窗口宽度随内容变宽（设备集合固定）", b["w"] > a["w"],
              f"{a['w']} → {b['w']}")

        ok = sum(1 for _, o, _ in RESULTS if o)
        print(f"\n=== {ok}/{len(RESULTS)} 通过 ===")
        return 0 if ok == len(RESULTS) else 2
    finally:
        kill()
        if os.path.exists(CFG_BAK):
            open(CFG, "w", encoding="utf-8").write(open(CFG_BAK, encoding="utf-8").read())
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.exit(main())

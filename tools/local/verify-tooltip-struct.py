"""阶段 0 真机验证：任务栏 widget 的每设备 hover tooltip 能否工作。

回答三问（gate）：
  Q1 分层任务栏子窗上，tooltip 能不能建出？
  Q2 逐项切换是否生效（每设备一条独立工具、独立文本）？
  Q3 提示会不会被任务栏盖住？

⛔ 本机环境铁律（MEMORY.md §3）：
  · 进程必须与「截图/探测」**同生命周期** ⇒ 启动与本脚本在同一条命令内
  · 本机**无法注入鼠标输入** ⇒ Q3 只能由人工肉眼确认，脚本**不假装**能判

⚠️ 本脚本首版连踩三个坑，全部是**探针**的错、不是实现的错（记录在此以免重蹈）：
  1. `TTM_GETTOOLCOUNT` 凭印象写成 1083 —— 实际是 **1037**（1083 是
     `TTM_GETCURRENTTOOLW`）⇒ 读出假的「0 条」，差点误判「注册失败」。
  2. tip 句柄从「系统里最后一个顶层 tip」取 —— 系统里有 **13 个** tip
     （含其它应用的）⇒ 连到了别人的窗。**正确做法：从应用自己的日志取句柄**
     （`[widget] tooltip: tip 窗已建 hwnd=0x…`）。
  3. 用 `TTM_ENUMTOOLS` 逐 id 探测 —— 它的语义是「lParam 指向数组、
     wParam = 容量」，**不是**「查某个 id」⇒ 读出假的「0 条」。
     改用语义无歧义的 `TTM_GETTEXTW(wParam=id)`。
"""

import ctypes
import ctypes.wintypes as wt
import glob
import io
import os
import re
import sys

sys.stdout.reconfigure(encoding="utf-8")

user32 = ctypes.WinDLL("user32", use_last_error=True)

# ── 类型 ────────────────────────────────────────────────────────────
class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


user32.FindWindowW.restype = wt.HWND
user32.FindWindowW.argtypes = [wt.LPCWSTR, wt.LPCWSTR]
user32.GetWindowRect.restype = wt.BOOL
user32.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
user32.GetClassNameW.argtypes = [wt.HWND, wt.LPWSTR, ctypes.c_int]
user32.EnumWindows.restype = wt.BOOL
user32.EnumChildWindows.restype = wt.BOOL
user32.IsWindow.restype = wt.BOOL
user32.IsWindow.argtypes = [wt.HWND]
user32.IsWindowVisible.restype = wt.BOOL
user32.IsWindowVisible.argtypes = [wt.HWND]
user32.SendMessageTimeoutW.restype = ctypes.c_uint
user32.SendMessageTimeoutW.argtypes = [wt.HWND, ctypes.c_uint, ctypes.c_size_t,
                                       ctypes.c_ssize_t, ctypes.c_uint,
                                       ctypes.c_uint, ctypes.POINTER(ctypes.c_size_t)]

# ── 常量（逐字核对自 windows-sys 0.61.2 Controls/mod.rs）─────────────
TTM_ENUMTOOLSW = 1082
TTM_GETCURRENTTOOLW = 1083
TTM_GETTEXTW = 1080
TTM_ACTIVATE = 1025
TTM_GETBUBBLESIZE = 1054
TTM_GETTOOLCOUNT = 1037          # ← 首版误写 1083
SMTO_ABORTIFHUNG = 0x0002

TOOLTIPS_CLASS = "tooltips_class32"
WIDGET_CLASS = "PeriTrayTaskbarWidget"
LOG_DIR = "D:/Code/PeriTray/src-tauri/target/debug/logs"

SEP = "-" * 66


def enum_children(parent):
    out = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(h, buf, 256)
        out.append((h, buf.value))
        return True

    user32.EnumChildWindows(parent, cb, 0)
    return out


def find_class_top(cls):
    hits = []

    @ctypes.WINFUNCTYPE(wt.BOOL, wt.HWND, wt.LPARAM)
    def cb(h, _):
        buf = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(h, buf, 256)
        if buf.value == cls:
            hits.append(h)
        return True

    user32.EnumWindows(cb, 0)
    return hits


def send_timeout(hwnd, msg, wp=0, lp=0):
    res = ctypes.c_size_t()
    r = user32.SendMessageTimeoutW(hwnd, msg, wp, lp, SMTO_ABORTIFHUNG, 2000,
                                   ctypes.byref(res))
    return (r != 0), res.value


def tool_count(tip):
    ok, v = send_timeout(tip, TTM_GETTOOLCOUNT)
    return v if ok else -1


def tool_texts(tip, n):
    """逐 id 取文本（TTM_GETTEXTW，语义无歧义）。"""
    out = []
    for uid in range(1, n + 1):
        buf = ctypes.create_unicode_buffer(512)
        ok, _ = send_timeout(tip, TTM_GETTEXTW, uid, ctypes.addressof(buf))
        if ok and buf.value:
            out.append((uid, buf.value))
    return out


def read_tip_hwnd_from_log():
    files = sorted(glob.glob(os.path.join(LOG_DIR, "*.log")), key=os.path.getmtime)
    if not files:
        return None
    txt = io.open(files[-1], encoding="utf-8", errors="replace").read()
    m = re.findall(r"tip 窗已建 hwnd=(0x[0-9a-fA-F]+)", txt)
    return m[-1] if m else None


def main():
    print("=" * 66)
    print("阶段 0 真机验证：任务栏 widget 的每设备 hover tooltip")
    print("=" * 66)

    found = find_class_top(TOOLTIPS_CLASS)
    print("")
    print("[P] 系统中 tip 窗（类=%s）: %d 个" % (TOOLTIPS_CLASS, len(found)))

    shell = user32.FindWindowW("Shell_TrayWnd", None)
    if not shell:
        print("❌ FAIL: Shell_TrayWnd 未找到（Explorer 未运行？）")
        return 2
    print("[P] Shell_TrayWnd = %#x" % shell)

    kids = enum_children(shell)
    wnames = [c for _, c in kids if c == WIDGET_CLASS]
    print("[P] 任务栏子窗 %d 个，其中 %s: %s" % (len(kids), WIDGET_CLASS, wnames))
    if not wnames:
        print("❌ FAIL: widget 未挂载到任务栏（先 cargo build + 启动并 pin 设备）")
        return 2
    widget = [h for h, c in kids if c == WIDGET_CLASS][0]
    r = RECT()
    user32.GetWindowRect(widget, ctypes.byref(r))
    print("[P] widget hwnd=%#x rect=(%d,%d)-(%d,%d) visible=%s"
          % (widget, r.left, r.top, r.right, r.bottom,
             bool(user32.IsWindowVisible(widget))))

    print("")
    print(SEP)
    print("Q1  分层任务栏子窗上，tooltip 能不能建出？")
    print(SEP)
    h = read_tip_hwnd_from_log()
    tip = int(h, 16) if h else 0
    if tip and not user32.IsWindow(tip):
        print("  ⚠️  日志里的 tip 句柄 %#x 已失效（进程重启过？）" % tip)
        tip = 0
    if not tip:
        print("  ❌ FAIL: 日志里无 tip 记录 ⇒ 原生 tooltip 在此形态下【未能建出】")
        return 1
    print("  ✅ tip 窗存在: %#x（owner = widget %#x）" % (tip, widget))

    print("")
    print(SEP)
    print("Q2  逐项切换是否生效？（每设备一条独立工具、独立文本）")
    print(SEP)
    n = tool_count(tip)
    print("  TTM_GETTOOLCOUNT = %d" % n)
    if n <= 0:
        print("  ❌ FAIL: 注册的工具数为 0 ⇒ tooltip 不会显示任何东西")
        return 1
    texts = tool_texts(tip, n)
    print("  取到 %d 条文本：" % len(texts))
    for uid, t in texts:
        print("    uId=%d  text=%r" % (uid, t))
    if len(texts) != n:
        print("  ❌ FAIL: 计数(%d) 与取到的文本数(%d) 不一致" % (n, len(texts)))
        return 1
    if len(set(t for _, t in texts)) != len(texts):
        print("  ⚠️  有重复文本（同名设备属正常，不判失败）")
    print("  ✅ %d 条均有非空文本 ⇒ 逐项切换的前提成立" % n)

    print("")
    print(SEP)
    print("Q3  提示会不会被任务栏盖住？")
    print(SEP)
    ok, sz = send_timeout(tip, TTM_GETBUBBLESIZE)
    if ok:
        w = ctypes.c_int(sz & 0xFFFFFFFF).value
        hgt = ctypes.c_int(sz >> 32).value
        print("  TTM_GETBUBBLESIZE = %d×%d（0 = 尚无内容可量）" % (w, hgt))
    tr = RECT()
    if user32.GetWindowRect(tip, ctypes.byref(tr)):
        print("  tip 窗 rect=(%d,%d)-(%d,%d) visible=%s"
              % (tr.left, tr.top, tr.right, tr.bottom,
                 bool(user32.IsWindowVisible(tip))))
    if not user32.IsWindowVisible(tip):
        print("  ⚠️  提示未显示 —— 本机【无法注入鼠标】，需人工把鼠标移到设备上肉眼确认。")
        print("     既不能判定为被盖住，也不能判定为正常。")

    print("")
    print("=" * 66)
    print("GATE 判定：Q1 ✅ / Q2 ✅ ⇒ 「分层 + 任务栏子窗 + 原生 tooltip」可行")
    print("        Q3 需人工肉眼确认（本机无鼠标注入能力）")
    print("=" * 66)
    return 0


if __name__ == "__main__":
    sys.exit(main())

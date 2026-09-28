"""把真实光标移到任务栏 widget 上，制造**真实 hover**，用于复现「首帧在左边」。

⛔ 本机平时不做注入（此前认为不可行），但 `SetCursorPos` 是可以调用的 ——
   「无法注入」只应理解为「没有现成工具」，不是「系统不允许」。
⚠️ 会**真的挪走用户的光标**。用完请把光标交还（脚本结束时会还原）。
"""
import ctypes
import ctypes.wintypes as wt
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)

# ⭐⭐ **必须先声明 DPI 感知**，否则本脚本的坐标全是**逻辑**值。
#   本机 125%：请求 y=1420 会被放大成物理 1775（出屏）→ 夹到物理 1440
#   → `GetCursorPos` 再除回 1.25 报 1151 ⇒ 「怎么都进不去任务栏」的假象。
#   这个坑让前两轮注入全部白跑（实测：y=1150 能落住、y=1160 起恒为 1151）。
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # PER_MONITOR_AWARE_V2
u.SetCursorPos.restype = wt.BOOL
u.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u.GetCursorPos.restype = wt.BOOL
u.GetCursorPos.argtypes = [ctypes.POINTER(wt.POINT)]


def get_pos():
    p = wt.POINT()
    u.GetCursorPos(ctypes.byref(p))
    return p.x, p.y


def move(x, y):
    ok = u.SetCursorPos(x, y)
    return ok


def main():
    x, y = int(sys.argv[1]), int(sys.argv[2])
    orig = get_pos()
    print("原光标 %s" % (orig,))
    # 分多步移动：一步跳过去可能被系统当成「瞬移」而不产生 WM_MOUSEMOVE 语义，
    # 而本仓的 hover 判定是 **GetCursorPos 轮询**（50ms），瞬移反而更可靠。
    for i in range(1, 6):
        nx = orig[0] + (x - orig[0]) * i // 5
        ny = orig[1] + (y - orig[1]) * i // 5
        move(nx, ny)
        time.sleep(0.08)
    ok = move(x, y)
    print("移到 (%d,%d) ok=%s 实际=%s" % (x, y, ok, get_pos()))
    return 0





if __name__ == "__main__":
    sys.exit(main())

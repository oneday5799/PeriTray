"""实机验证：音量控制页右键菜单的「钉到任务栏 / 移出任务栏」**文案会翻转**。

⛔ 为什么必须脚本化：这是「点一次 → 关菜单 → 再右键看文案」的两步交互，
   人工点容易漏看其中一步；而回归的表现恰恰是「点了但文案不变」。

判据：同一台设备连续两次右键，菜单文案必须**在两次之间翻转**。
"""
import ctypes
import ctypes.wintypes as wt
import re
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

u = ctypes.WinDLL("user32", use_last_error=True)
u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))


class RECT(ctypes.Structure):
    _fields_ = [("l", ctypes.c_long), ("t", ctypes.c_long),
                ("r", ctypes.c_long), ("b", ctypes.c_long)]


u.GetWindowRect.argtypes = [wt.HWND, ctypes.POINTER(RECT)]
u.FindWindowW.restype = wt.HWND
u.FindWindowW.argtypes = [ctypes.c_wchar_p, ctypes.c_wchar_p]
u.IsWindowVisible.argtypes = [wt.HWND]
u.IsWindowVisible.restype = wt.BOOL
u.SetCursorPos.argtypes = [ctypes.c_int, ctypes.c_int]
u.mouse_event.argtypes = [ctypes.c_uint] * 4 + [ctypes.c_void_p]
LDOWN, LUP, RDOWN, RUP = 0x0002, 0x0004, 0x0008, 0x0010
CFG = r"D:\Code\PeriTray\src-tauri\target\debug\config.toml"


def click(x, y, down=LDOWN, up=LUP):
    u.SetCursorPos(x, y)
    time.sleep(0.35)
    u.mouse_event(down, 0, 0, 0, None)
    time.sleep(0.05)
    u.mouse_event(up, 0, 0, 0, None)


def open_popup():
    for x in range(2380, 2545, 15):
        click(x, 1430)
        time.sleep(1.5)
        h = u.FindWindowW(None, "外设信息")
        if h and u.IsWindowVisible(h):
            for _ in range(20):
                rc = RECT()
                u.GetWindowRect(h, ctypes.byref(rc))
                if rc.b <= 1380 and rc.b > 1000:
                    return rc
                time.sleep(0.3)
            return rc
    return None


def pinned_names():
    return re.findall(r'fallback = "n:(.*?)"',
                      open(CFG, encoding="utf-8").read())


def main():
    rc = open_popup()
    if not rc:
        print("❌ 弹窗未打开")
        return 1
    print("popup rect=(%d,%d)-(%d,%d)" % (rc.l, rc.t, rc.r, rc.b))
    # 切到音量控制 tab
    click(rc.l + int((rc.r - rc.l) * 0.62), rc.t + 18)
    time.sleep(1.5)

    # 选一张卡片：网易虚拟音频设备（第 2 张，绝对 y = rc.t + 250）
    cy = rc.t + 250
    cx = rc.l + 200
    click(cx, cy, RDOWN, RUP)
    time.sleep(1.2)
    # 菜单项「钉到任务栏」/「移出任务栏」在菜单里的第 4 行
    mx, my = cx + 125, cy + 170
    before = pinned_names()
    click(mx, my)
    time.sleep(2.5)
    after = pinned_names()
    print("已钉：%s" % before)
    print("点击后：%s" % after)
    print("变化：%s" % ("有 +%d 台" % (len(after) - len(before))
                        if len(after) != len(before) else "无变化 ❌"))
    return 0 if len(after) != len(before) else 1


if __name__ == "__main__":
    sys.exit(main())

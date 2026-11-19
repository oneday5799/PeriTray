"""端到端验收：逐个设备注入光标，断言 tooltip 的**索引**与**落位**都对。

⚠️ 本脚本会**真的挪动光标**，开始前记录原位置，结束时交还。
⚠️ 验收期间请**不要**动鼠标（否则读到的是你的移动，不是脚本的）。

用法：verify-tooltip-hover.py <日志路径>
"""
import re
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

sys.path.insert(0, "D:/Code/PeriTray/tools/local")
from move_cursor import get_pos, move  # noqa: E402

LOG = sys.argv[1]
# 设备命中的屏幕区间（日志实测：#0=1149 #1=1217 #2=1285 #3=1353，各宽 58）
ANCHORS = [1149, 1217, 1285, 1353]
ITEM_W = 58
Y = 1410
SETTLE = 1.2  # > 500ms 延迟，够 tooltip 出现


def tail(n=200):
    with open(LOG, encoding="utf-8", errors="replace") as f:
        return f.read().splitlines()[-n:]


def show_for(idx):
    """找 idx 最后一次成功落位。"""
    pat = re.compile(r"显示 #%d 「.*?」气泡=(\d+)×(\d+) 窗=\((-?\d+),(-?\d+)\)" % idx)
    for line in reversed(tail()):
        m = pat.search(line)
        if m:
            return int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
    return None


def main():
    orig = get_pos()
    print("原光标 %s（结束会交还）\n" % (orig,))
    ok = []
    try:
        for i, ax in enumerate(ANCHORS):
            x = ax + ITEM_W // 2
            # 先离开 widget，保证是「首次出现」而非「已在显示」
            move(x, Y)
            time.sleep(0.25)
            move(200, 200)  # 移开
            time.sleep(0.35)
            move(x, Y)
            time.sleep(SETTLE)
            real = get_pos()
            got = show_for(i)
            if got is None:
                print("[%d] ❌ 无落位记录（光标实际 %s）" % (i, real))
                ok.append(False)
                continue
            bw, bh, wx, wy = got
            # 期望：气泡水平居中于该设备的命中区
            exp_cx = ax + ITEM_W // 2
            act_cx = wx + 18 + bw // 2  # 18 = 画布边距；bubble 从 +18 开始
            # 允许 2px：居中取整
            good = abs(act_cx - exp_cx) <= 2
            ok.append(good)
            print("[%d] %s 光标%s 气泡%d×%d 窗=(%d,%d) 中心%d 期望中心%d %s"
                  % (i, "✅" if good else "❌", real, bw, bh, wx, wy,
                     act_cx, exp_cx, "" if good else "← 落位错位"))
            time.sleep(0.3)
    finally:
        move(*orig)
        print("\n光标已交还 %s" % (get_pos(),))
    print("\n%s  %d/%d 通过"
          % ("✅ 全部通过" if all(ok) else "❌ 有失败", sum(ok), len(ok)))
    return 0 if all(ok) else 1


if __name__ == "__main__":
    sys.exit(main())

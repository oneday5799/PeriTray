"""验收「跨设备切换严格遵循注入顺序、无回退」。

判据：**整段日志**（本轮偏移之后）的「显示 #N」折叠序列
      （相邻去重）必须**严格等于**注入计划。
- 严格相等 ⇒ 既没有凭空冒出别的设备（含设备 0），也没有漏掉任何一个。
- ⛔ 不按「时间窗口」切分：日志时间戳是本地时钟字符串，与 `time.time()`
  不同基准（实测恒切出空/错窗口）。⛔ 也不扫全文件：会混进上一轮记录。
  ⇒ 用**文件字节偏移**划出本轮，再整体折叠。
- ⚠️ 收尾 `move(*orig)` 交还光标会触发一次落位 ⇒ 必须在**记下结束偏移后**
  再交还，并把解析限制在 `[frm:end)`。
"""
import os
import re
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")
sys.path.insert(0, "D:/Code/PeriTray/tools/local")
from move_cursor import get_pos, move  # noqa: E402

LOG = sys.argv[1]
ANCHORS = [1149, 1217, 1285, 1353]
ITEM_W = 58
Y = 1410
PLAN = [0, 1, 2, 3, 2, 1]


def fold(frm, to):
    with open(LOG, "rb") as f:
        f.seek(frm)
        raw = f.read(to - frm).decode("utf-8", "replace")
    seq = [int(m.group(1)) for m in
           (re.search(r"tooltip: 显示 #(\d+)", l) for l in raw.splitlines()) if m]
    out = [seq[0]] if seq else []
    for v in seq[1:]:
        if v != out[-1]:
            out.append(v)
    return out


def main():
    orig = get_pos()
    frm = os.path.getsize(LOG) if os.path.exists(LOG) else 0
    print("原光标 %s（本轮区间 [%d, end)）" % (orig, frm))
    try:
        for i in PLAN:
            move(300, 300)               # 离开 widget，让状态复位
            time.sleep(0.40)
            move(ANCHORS[i] + ITEM_W // 2, Y)
            time.sleep(0.70)             # > 500ms 首次延迟
    finally:
        end = os.path.getsize(LOG)       # ⛔ 收尾动作之前先记结束偏移
        move(*orig)
    time.sleep(0.3)
    hops = fold(frm, end)
    print("注入计划      ：%s" % (PLAN,))
    print("日志折叠序列  ：%s" % (hops,))
    good = hops == PLAN
    print("\n%s  严格相等" % ("✅" if good else "❌"))
    if not good:
        print("  多出：%s" % ([v for v in hops if v not in PLAN],))
        print("  缺失：%s" % ([v for v in PLAN if v not in hops],))
    print("光标已交还 %s" % (get_pos(),))
    return 0 if good else 1


if __name__ == "__main__":
    sys.exit(main())

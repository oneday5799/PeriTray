"""按指定时刻注入光标，复现「首帧在左边」。

用法：hover-repro.py <注入延迟ms> <x> <y>
"""
import subprocess
import sys
import time

sys.stdout.reconfigure(encoding="utf-8")

MOVE = "D:/Code/PeriTray/tools/local/move-cursor.py"


def main():
    delay = int(sys.argv[1]) / 1000.0
    x, y = int(sys.argv[2]), int(sys.argv[3])
    time.sleep(delay)
    subprocess.run(
        [sys.executable, MOVE, str(x), str(y)],
        capture_output=True, text=True,
    )
    print("t=%.2fs 注入 (%d,%d)" % (delay, x, y))


if __name__ == "__main__":
    main()

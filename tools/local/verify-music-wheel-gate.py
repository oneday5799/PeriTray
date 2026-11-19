# -*- coding: utf-8 -*-
"""注入验证：音乐面板下滚轮**不得**触发音量调整。

判据（可证伪）：注入一次真实滚轮 ⇒ 日志必须出现
  「滚轮未受理: 当前面板=Some(Music)」
且**不得**出现「滚轮音量 id=」或「滚轮调音量」。
删掉面板闸的话，这两条会同时不成立（滚轮会去调设备音量）。
"""
import ctypes
import os
import re
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".musicbak"
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print("  [%s] %s  %s" % ("OK" if ok else "FAIL", name, detail))


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def io_write(p, s):
    import io
    io.open(p, "w", encoding="utf-8", newline="").write(s)


def kill_all():
    subprocess.run(["taskkill", "-F", "-IM", "PeriTray.exe"],
                   capture_output=True, shell=False)
    time.sleep(2)


def newest_log():
    d = os.path.join(WD, "logs")
    if not os.path.isdir(d):
        return None
    logs = [os.path.join(d, f) for f in os.listdir(d) if f.endswith(".log")]
    return max(logs, key=os.path.getmtime) if logs else None


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml")
        return
    io_write(CFG_BAK, io_read(CFG))
    try:
        s = io_read(CFG)
        s = re.sub(r'^log_level = .*$', 'log_level = "verbose"', s, flags=re.M)
        # 打开音乐开关
        if re.search(r"^taskbar_music_enabled = ", s, flags=re.M):
            s = re.sub(r"^taskbar_music_enabled = .*$", 'taskbar_music_enabled = true',
                       s, flags=re.M)
        else:
            s = s.rstrip() + "\ntaskbar_music_enabled = true\n"
        io_write(CFG, s)

        kill_all()
        for f in os.listdir(os.path.join(WD, "logs")):
            if f.endswith(".log"):
                os.remove(os.path.join(WD, "logs", f))
        env = dict(os.environ)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
            "--disable-gpu-sandbox --remote-debugging-port=9222"
        subprocess.Popen([EXE], cwd=WD, env=env)
        time.sleep(14)

        log = newest_log()
        if not log:
            check("应用启动", False)
            return
        txt = io_read(log)
        # ⚠️ 取**最后一条**而不是第一条：应用刚启动时 SMTC 会话可能还没就绪
        # （第一轮显示 Devices），一两秒后才切到 Music。判据若 grep 第一条就会
        # 把「启动瞬间的过渡态」当成「音乐面板没起来」—— 我第一版就踩了这个。
        rows = re.findall(r"面板判据: 显示=(\S+)", txt)
        latest = rows[-1] if rows else "无面板判据日志"
        check("音乐面板已激活（取最新一条判据）", "Music" in latest,
              "最新=%s / 共 %d 条" % (latest, len(rows)))

        # 找 widget 的屏幕 x（详细级「定位」日志里有 rel_x；y 取任务栏中线）
        rows = re.findall(r"定位: .*→ rel_x=(\d+)", txt)
        if not rows:
            check("读到 widget 位置", False)
            return
        rel_x = int(rows[-1])
        # 任务栏客户区宽度取自同一条日志的 area=(0,w=..)
        area = re.findall(r"area=\(0,w=(\d+)\)", txt)
        tb_w = int(area[-1]) if area else 2560
        cx, cy = rel_x + 60, 1385 + 25
        print("  注入点 =", (cx, cy), "（rel_x=%d, 任务栏宽=%d）" % (rel_x, tb_w))

        u = ctypes.windll.user32
        try:
            u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
        except Exception:
            pass
        u.SetCursorPos(int(cx), int(cy))
        time.sleep(1.2)  # 等 hover 底衬画上
        before = io_read(newest_log())
        for _ in range(3):
            u.mouse_event(0x0800, 0, 0, 120, 0)
            time.sleep(0.4)
        time.sleep(2.5)
        after = io_read(newest_log())

        rejected = "滚轮未受理: 当前面板=Some(Music)" in after
        changed = after.count("滚轮音量 id=") - before.count("滚轮音量 id=")
        check("音乐面板下滚轮被**明确拒绝**", rejected,
              "找到「滚轮未受理: 当前面板=Some(Music)」" if rejected
              else "未找到拒绝日志（判据可能失效）")
        check("音乐面板下滚轮**未**改动任何设备音量", changed == 0,
              "新增音量写入 %d 条" % changed)
    finally:
        kill_all()
        io_write(CFG, io_read(CFG_BAK))
        os.remove(CFG_BAK)
        print("  (config.toml 已还原)")


if __name__ == "__main__":
    main()
    ok = sum(1 for _, o, _ in RESULTS if o)
    print("\n=== %d/%d 通过 ===" % (ok, len(RESULTS)))
    sys.exit(0 if ok == len(RESULTS) else 1)

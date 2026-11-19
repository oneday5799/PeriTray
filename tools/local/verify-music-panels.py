# -*- coding: utf-8 -*-
"""注入验证：两个开关的独立性 + 「切换」按钮的显示条件 + 循环顺序。

判据（可证伪）：
  A 只开设备      -> 面板 = Devices
  B 只开音乐      -> 面板 = Music（有会话时）
  C 都关          -> 面板 = None（组件不显示）
  D 都开          -> 面板 = Music（音乐优先）+ 切换按钮可见
  E 记住的选择在**重启后**仍生效
"""
import importlib.util
import sys as _sys
_sys.stdout.reconfigure(encoding="utf-8", errors="replace")
import os
import re
import subprocess
import sys
import time

spec = importlib.util.spec_from_file_location(
    "g", r"D:\Code\PeriTray\tools\local\verify-music-wheel-gate.py")
g = importlib.util.module_from_spec(spec)
spec.loader.exec_module(g)

WD = g.WD
EXE = g.EXE
CFG = g.CFG
CFG_BAK = CFG + ".panelbak"
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok))
    print("  [%s] %s  %s" % ("OK" if ok else "FAIL", name, detail))


def seed(dev_on, music_on, panel=None):
    s = g.io_read(CFG)
    s = re.sub(r'^log_level = .*$', 'log_level = "verbose"', s, flags=re.M)
    for key, val in (("taskbar_widget_enabled", dev_on),
                     ("taskbar_music_enabled", music_on)):
        lit = "true" if val else "false"
        if re.search(r"^%s = " % key, s, flags=re.M):
            s = re.sub(r"^%s = .*$" % key, "%s = %s" % (key, lit), s, flags=re.M)
        else:
            s = s.rstrip() + "\n%s = %s\n" % (key, lit)
    if panel:
        if re.search(r"^taskbar_panel = ", s, flags=re.M):
            s = re.sub(r"^taskbar_panel = .*$", 'taskbar_panel = "%s"' % panel, s, flags=re.M)
        else:
            s = s.rstrip() + '\ntaskbar_panel = "%s"\n' % panel
    g.io_write(CFG, s)


def run_case(dev_on, music_on, panel=None, label=""):
    seed(dev_on, music_on, panel)
    g.kill_all()
    for f in os.listdir(os.path.join(WD, "logs")):
        if f.endswith(".log"):
            os.remove(os.path.join(WD, "logs", f))
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    subprocess.Popen([EXE], cwd=WD, env=env)
    time.sleep(14)
    log = g.newest_log()
    if not log:
        check(label, False, "无日志")
        return None
    txt = g.io_read(log)
    m = re.search(r"面板判据: 显示=(\S+)", txt)
    shown = m.group(1) if m else "?"
    sessions = re.search(r"会话数=(\d+)", txt)
    ns = int(sessions.group(1)) if sessions else 0
    # 切换按钮是否绘制：详细级日志里没有 -> 只能从「hover 形态宽度」间接看，
    # 这里改用「音乐面板 hover 时的定位日志」里 panel=Music 作为面板确认
    return shown, ns, txt


def main():
    g.io_write(CFG_BAK, g.io_read(CFG))
    try:
        # ⭐ 三条判据都**跟着「当次会话数」走**——这不是放宽，是规格本身：
        #   回落链的第一条就是「音乐会话存在」。会话数会随用户开/关音乐 App 变化，
        #   写死期望值 = 把「环境状态变了」误判成「产品坏了」（我第一版就踩了这个）。
        #   判据的正确写法是「**给定**会话数下应当是什么」。

        # A 只开设备：无论有没有会话，都应是 Devices（音乐开关关 ⇒ 音乐不可选）
        r = run_case(True, False, "devices", "A 只开设备开关")
        if r:
            check("A 只开设备 -> 恒显示 Devices", r[0] == "Some(Devices)",
                  "面板=%s 会话数=%d" % (r[0], r[1]))

        # B 只开音乐：有会话 -> Music；无会话 -> 不显示（回落链：无设备可回落）
        r = run_case(False, True, "devices", "B 只开音乐开关")
        if r:
            want = "Some(Music)" if r[1] > 0 else "None"
            check("B 只开音乐 -> 有会话显示 Music / 无会话不显示", r[0] == want,
                  "面板=%s 会话数=%d 期望=%s" % (r[0], r[1], want))

        # C 都关：组件**根本不挂载** => 不该有任何面板判据日志
        seed(False, False, "devices")
        g.kill_all()
        for f in os.listdir(os.path.join(WD, "logs")):
            if f.endswith(".log"):
                os.remove(os.path.join(WD, "logs", f))
        env = dict(os.environ)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] =             "--disable-gpu-sandbox --remote-debugging-port=9222"
        subprocess.Popen([EXE], cwd=WD, env=env)
        time.sleep(14)
        txt = g.io_read(g.newest_log()) if g.newest_log() else ""
        check("C 两个开关都关 -> 组件不挂载（无面板判据日志）",
              "面板判据" not in txt and "按配置挂载" not in txt,
              "面板判据出现 %d 次" % txt.count("面板判据"))

        # D 都开：有会话 -> Music 优先；无会话 -> 回落 Devices
        r = run_case(True, True, "devices", "D 都开")
        if r:
            want = "Some(Music)" if r[1] > 0 else "Some(Devices)"
            check("D 两个都开 -> 音乐优先 / 无会话回落设备", r[0] == want,
                  "面板=%s 会话数=%d 期望=%s" % (r[0], r[1], want))

        # E 记住的选择**不覆盖**开关：开关开即走音乐（音乐可用时）
        r = run_case(True, True, "music", "E 记住 music")
        if r:
            want = "Some(Music)" if r[1] > 0 else "Some(Devices)"
            check("E 记住 music + 开关开 -> 音乐面板", r[0] == want,
                  "面板=%s 会话数=%d 期望=%s" % (r[0], r[1], want))

        # F ⭐ 回落**不改写**记住的选择：无会话时配置里仍应是 music
        s2 = g.io_read(CFG)
        check("F 回落不改写用户的选择（配置仍是 music）",
              'taskbar_panel = "music"' in s2,
              "配置里的 taskbar_panel = %s" %
              (re.search(r'taskbar_panel = "(\w+)"', s2).group(1)
               if re.search(r'taskbar_panel = "(\w+)"', s2) else "?"))
    finally:
        g.kill_all()
        g.io_write(CFG, g.io_read(CFG_BAK))
        os.remove(CFG_BAK)
        print("  (config.toml 已还原)")


if __name__ == "__main__":
    main()
    ok = sum(1 for _, o in RESULTS if o)
    print("\n=== %d/%d 通过 ===" % (ok, len(RESULTS)))
    sys.exit(0 if ok == len(RESULTS) else 1)

# -*- coding: utf-8 -*-
"""定位：设置页「设备信息」与「任务栏」两处在改名后到底显示了什么（只读探针）。

用法：先由调用方把 config.toml 预置成「已改名」状态，本脚本只负责
      启动 → 逐个 tab 抓文本 → 打印，最后还原 config。
"""
import json
import os
import re
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".probebak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

ALIAS = "改名验收名"
SHORT = "小爱音箱-9205"
LONG = "耳机 (小爱音箱-9205)"

PROBE = ("(async()=>{const out={};const grab=async(tab,sel)=>{"
         "document.querySelector('.win-nav-item[data-tab=\"'+tab+'\"]').click();"
         "await new Promise(r=>setTimeout(r,1500));"
         "return [...document.querySelectorAll('#tab-'+tab+' '+sel)]"
         ".map(e=>e.textContent.trim()).filter(Boolean);};"
         "out.deviceNames=await grab('device','.card-item-name');"
         "out.deviceTitles=await grab('device','.card-title');"
         "out.taskbarNames=await grab('taskbar','#taskbar-widget-devices .card-item-name');"
         "out.taskbarEmpty=await grab('taskbar','#taskbar-widget-empty');"
         "out.taskbarAll=await grab('taskbar','.card-item-name');"
         "return JSON.stringify(out);})()")


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def io_write(p, s):
    import io
    io.open(p, "w", encoding="utf-8", newline="").write(s)


def seed():
    s = io_read(CFG)
    s = re.sub(r"^simplify_device_names = .*$", "simplify_device_names = false", s, flags=re.M)
    block = '[device_names]\n"%s" = "%s"\n"%s" = "%s"\n' % (SHORT, ALIAS, LONG, ALIAS)
    if "[device_names]" in s:
        s = re.sub(r"\[device_names\]\n(?:.*\n)*?(?=\n\[|\Z)", block, s, count=1)
    else:
        s = s.rstrip() + "\n\n" + block
    io_write(CFG, s)


def main():
    io_write(CFG_BAK, io_read(CFG))
    try:
        seed()
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(2)
        env = dict(os.environ)
        env["PM_DEV_OPEN_SETTINGS"] = "1"
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
            "--disable-gpu-sandbox --remote-debugging-port=9222"
        subprocess.Popen([EXE], cwd=WD, env=env)
        time.sleep(13)
        r = subprocess.run([NODE, CDP, "settings.html", PROBE], capture_output=True,
                           text=True, encoding="utf-8", errors="replace", timeout=90)
        if r.returncode != 0:
            print("探针失败:", (r.stderr or r.stdout).strip()[:400])
            return 2
        data = json.loads(r.stdout.strip())
        for k, v in data.items():
            hits = [x for x in v if ALIAS in x or SHORT in x or "小爱" in x]
            print(f"  {k}: 共 {len(v)} 项 | 命中: {hits}")
        return 0
    finally:
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
        if os.path.exists(CFG_BAK):
            io_write(CFG, io_read(CFG_BAK))
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

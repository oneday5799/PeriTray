# -*- coding: utf-8 -*-
"""定位：设置页两个 tab 在**运行时**被外部改名后是否自动同步。

⚠️ 与 `probe-settings-names.py` 的区别：那个是**启动前**把别名写进 config.toml
   （应用启动时读入）⇒ 只证明「加载路径正确」。这个是**运行时**改名
   （模拟用户在弹出窗口改完名后切到设置窗口），走的是
   `rename_device` → `config-changed` → 设置页各 tab 的刷新链路。

⚠️ 改名动作**故意从 settings.html 发起**：设置窗口可见时，隐藏的弹出窗口页面
   发起的 `invoke` 不返回（实测，见本文件末尾备注），所以只能用可见的那页驱动。
   事件（`config-changed` / `audio-devices-changed`）由后端统一发出，
   与发起方无关 ⇒ 测的是同一条链路。
"""
import json
import os
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".rtsyncbak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

SHORT = "小爱音箱-9205"
LONG = "耳机 (小爱音箱-9205)"
ALIAS = "运行时改名"


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def io_write(p, s):
    import io
    io.open(p, "w", encoding="utf-8", newline="").write(s)


def cdp(expr, timeout=60, retries=1):
    last = None
    for _ in range(retries + 1):
        try:
            r = subprocess.run([NODE, CDP, "settings.html", expr], capture_output=True,
                               text=True, encoding="utf-8", errors="replace", timeout=timeout)
            if r.returncode != 0:
                raise RuntimeError((r.stderr or r.stdout).strip()[:200])
            return json.loads(r.stdout.strip())
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(2)
    raise RuntimeError(str(last))


def port_alive():
    import urllib.request
    try:
        with urllib.request.urlopen("http://127.0.0.1:9222/json", timeout=1) as r:
            return r.status == 200
    except Exception:  # noqa: BLE001
        return False


def kill_all():
    # ⚠️ 必须**先无条件杀一次**：上一次的实例可能是**不带调试端口**启动的
    #    ⇒ port_alive() 为 False ⇒ 「等端口空出来」的写法会直接返回、根本没杀
    #    ⇒ 新实例撞上单实例守卫立刻退出（本文件第一次跑就是这样失败的）。
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    for _ in range(20):
        if not port_alive():
            return
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)


# 读两个 tab 的设备名
PROBE = ("(async()=>{const grab=async(tab,sel)=>{"
         "document.querySelector('.win-nav-item[data-tab=\"'+tab+'\"]').click();"
         "await new Promise(r=>setTimeout(r,1500));"
         "return [...document.querySelectorAll('#tab-'+tab+' '+sel)]"
         ".map(e=>e.textContent.trim()).filter(Boolean);};"
         "const cfg=await window.__TAURI__.core.invoke('get_config');"
         "return JSON.stringify({names:cfg.device_names,"
         "dev:await grab('device','.card-item-name'),"
         "taskbar:await grab('taskbar','#taskbar-widget-devices .card-item-name')});})()")

# 运行时改名（从可见的设置页发起）
RENAME = ("(async()=>{const inv=window.__TAURI__.core.invoke;"
          "await inv('rename_device',{original:'%s',newName:'%s'});"
          "return JSON.stringify('ok');})()") % (SHORT, ALIAS)


def main():
    io_write(CFG_BAK, io_read(CFG))
    try:
        kill_all()
        time.sleep(1)
        env = dict(os.environ)
        env["PM_DEV_OPEN_SETTINGS"] = "1"
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
            "--disable-gpu-sandbox --remote-debugging-port=9222"
        proc = subprocess.Popen([EXE], cwd=WD, env=env)
        for k in range(30):
            time.sleep(2)
            if proc.poll() is not None:
                raise RuntimeError("应用启动后退出")
            try:
                cdp("(async()=>{await window.__TAURI__.core.invoke('get_config');"
                    "return JSON.stringify('ok');})()", timeout=20, retries=0)
                print(f"  (IPC 就绪，轮询 {k + 1} 次)")
                break
            except Exception:  # noqa: BLE001
                continue

        before = cdp(PROBE)
        print("  改名前 device 区:", before.get("dev"))
        print("  改名前 taskbar 区:", before.get("taskbar"))
        print("  改名前 device_names:", json.dumps(before.get("names"), ensure_ascii=False))

        print("  → 运行时改名 …")
        cdp(RENAME)
        time.sleep(3.0)

        after = cdp(PROBE)
        print("  改名后 device 区:", after.get("dev"))
        print("  改名后 taskbar 区:", after.get("taskbar"))
        print("  改名后 device_names:", json.dumps(after.get("names"), ensure_ascii=False))

        results = [
            ("后端已写入别名",
             after.get("names", {}).get(SHORT) == ALIAS),
            ("设置页·设备信息区同步为别名",
             any(ALIAS in x for x in after.get("dev", []))),
            ("设置页·任务栏清单同步为别名",
             any(ALIAS in x for x in after.get("taskbar", []))),
            ("两处都不再残留旧别名",
             not any("57777" in x for x in after.get("dev", []) + after.get("taskbar", []))),
        ]
        ok = 0
        for name, good in results:
            print(f"  {'[OK]  ' if good else '[FAIL]'} {name}")
            ok += 1 if good else 0
        print(f"=== {ok}/{len(results)} 通过 ===")
        return 0 if ok == len(results) else 2
    finally:
        kill_all()
        if os.path.exists(CFG_BAK):
            io_write(CFG, io_read(CFG_BAK))
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

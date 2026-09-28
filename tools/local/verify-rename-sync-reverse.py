# -*- coding: utf-8 -*-
"""验收：**音量控制页 → 设备信息页**方向的重命名同步 + 「恢复默认」双向归位。

⛔ 缺陷背景（2026-09-28 用户二次报）：第一轮只修了「设备页 → 音量页」，
   反向仍不同步。根因**不在解析器**而在**订阅**：
   `rename_device` 只**发事件**不回调页面，而 `popup-devices.js` 原先
   **完全没有 `config-changed` 订阅**（`deviceNames` 只在 `loadDevices` /
   快照水合时刷新）⇒ 本页要等下次整表重拉才更新；音量页则订阅了
   `audio-devices-changed` ⇒ 单向同步。

判据锚在**真实渲染出来的 DOM 文本**（`#device-list .device-name`），
不是解析器函数：解析器对 ≠ 页面刷新了。第二轮修的正是后者，
只测解析器会漏掉这次的真实缺陷。
"""
import json
import os
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".ren2bak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

LONG = "耳机 (小爱音箱-9205)"
SHORT = "小爱音箱-9205"
ALIAS = "SYNC验收名"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def port_9222_alive():
    """调试端口是否还有人在监听（= 上一个实例是否真的退干净了）。

    ⛔ 为什么不用 `tasklist` 数进程：Python 捕获 `tasklist` 输出时**不按本机
    locale 解码**（重定向场景常是 UTF-16），`count("peritray.exe")` 恒为 0
    ⇒ 把正常启动误判成「应用已消失」。改用与编码无关的 HTTP 端点判据。
    """
    import urllib.request
    try:
        with urllib.request.urlopen("http://127.0.0.1:9222/json", timeout=1) as r:
            return r.status == 200
    except Exception:  # noqa: BLE001
        return False


def kill_all(tries=20):
    """⚠️ 必须**反复杀到真的没有为止**再启动下一个实例。

    踩坑记录：`taskkill /F` 在 Git-Bash 里会被路径化成 `F:/` 而**静默失败**
    （要么用 `-F` 形式，要么像这里走 subprocess 不经 shell）⇒ 曾经同时活着
    多个实例，9222 端口归最早那个 ⇒ CDP 探到的是**上一个进程的页面**：
    它能执行 JS（`1+1` 有结果），但 IPC 通道已随进程被杀而断裂，
    任何 `invoke` 永不 settle。**看着像代码坏了，其实是探到了僵尸页面。**
    """
    for _ in range(tries):
        if not port_9222_alive():
            return
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
    raise RuntimeError("9222 端口仍被占用：旧实例没退干净")


def wait_ipc(tries=30):
    """⛔ 孤儿 target 陷阱：`taskkill` 后 WebView2 子进程未退、调试端口未释放时，
    `/json` 里可能还挂着上个进程的页面 —— 它能执行 JS，但 IPC 通道已断
    ⇒ 任何 invoke 永不 settle。必须等「进程真没了 + IPC 真通了」再断言。
    """
    for k in range(tries):
        try:
            # ⚠️ 本文件的 `cdp` 只有 (expr, timeout) 两个形参、页面写死在内部 ⇒ 
            #    这里**不能**按 (page, expr, …) 调用（曾这么写，每次轮询都抛
            #    TypeError 又被 except 吞掉 ⇒ 表现为「IPC 永远不就绪」）。
            cdp("(async()=>{await window.__TAURI__.core.invoke('get_config');"
                 "return JSON.stringify('ok');})()", timeout=20)
            print(f"  (IPC 就绪，轮询 {k + 1} 次)")
            return
        except Exception:  # noqa: BLE001
            time.sleep(2)
    raise RuntimeError("IPC 始终未就绪")


def cdp(expr, timeout=90):
    r = subprocess.run([NODE, CDP, "popup.html", expr], capture_output=True,
                       text=True, encoding="utf-8", errors="replace", timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return json.loads(r.stdout.strip())


def rename(original, new_name):
    return ("(async()=>{await window.__TAURI__.core.invoke("
            "'rename_device',{original:'%s',newName:'%s'});"
            "await new Promise(r=>setTimeout(r,1200));return JSON.stringify('ok');})()"
            ) % (original, new_name)


# 读**渲染后的 DOM** + 后端 map + 解析器（simplify 两种取值都读，覆盖「各自原命名」）
PROBE = ("(async()=>{const c=await window.__TAURI__.core.invoke('get_config');"
         "const n=c.device_names||{};const s=c.simplify_device_names!==false;"
         "const cards=[...document.querySelectorAll('#device-list .device-name')]"
         ".map(e=>e.textContent);"
         "const ac=[...document.querySelectorAll('.audio-device-name')]"
         ".map(e=>e.textContent);"
         "return JSON.stringify({map:n,cards:cards,audio:ac,"
         "deviceRaw:window.getDisplayName({name:'%s'},n),"
         "audioRaw:window.formatDeviceName('%s',n,false),"
         "audioSim:window.formatDeviceName('%s',n,s)});})()") % (SHORT, LONG, LONG)


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml")
        return 1
    open(CFG_BAK, "w", encoding="utf-8").write(open(CFG, encoding="utf-8").read())
    try:
        kill_all()
        env = dict(os.environ)
        env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
            "--disable-gpu-sandbox --remote-debugging-port=9222"
        subprocess.Popen([EXE], cwd=WD, env=env)
        time.sleep(10)
        wait_ipc()

        # ── ⭐ 先强制一次**实查**，否则断言会落在快照数据上 ──
        #   弹窗首屏走 `hydrateFromSnapshot()`（localStorage 秒显），快照里的设备名
        #   可能是**旧值**（实测：小爱音箱卡片显示快照里的 `小爱音箱-920`，
        #   而 `get_devices` 早已是 `小爱音箱-9205`）⇒ 拿它断言会得到与本缺陷
        #   无关的假失败（快照名与音量页端点名对不上）。
        #   归并判据依赖「设备页名字 == 音量页短名」，只有在**实查**数据上才成立。
        # ⛔ 必须**轮询等 DOM 与实查数据一致**再断言：弹窗首屏走快照
        #   （localStorage 秒显），而首次实查要几百毫秒~数秒（2.4G 电量查询，
        #   鼠标休眠时更慢）⇒ 固定 sleep 会读到快照里的**旧设备名**
        #   （实测卡片 `小爱音箱-920` vs 实查 `小爱音箱-9205`），
        #   那与本缺陷无关，却会让判据变成随机红。
        # ⚠️ 期望值必须用**显示名**（`getDisplayName`，含别名）而不是 `d.name`：
        #    配置里若已有别名（本机就有一个），卡片上显示的是别名，拿原名去比
        #    **必然不等** ⇒ 该检查恒红，且与被测机制无关（这个坑让我误判过两次）。
        settled = cdp("(async()=>{const inv=window.__TAURI__.core.invoke;"
                      "const cfg=await inv('get_config');"
                      "const want=(await inv('get_devices'))"
                      ".map(d=>getDisplayName(d,cfg.device_names||{}));"
                      "const read=()=>[...document.querySelectorAll('#device-list .device-name')]"
                      ".map(e=>e.textContent);"
                      "for(let i=0;i<40;i++){await loadDevices();"
                      "await new Promise(r=>setTimeout(r,700));"
                      "const got=read();"
                      "if(got.length===want.length&&got.every(v=>want.indexOf(v)>=0))"
                      "return JSON.stringify({ok:true,try:i+1,got});}"
                      "return JSON.stringify({ok:false,want:want,got:read()});})()")
        print("  等 DOM 追平实查:", json.dumps(settled, ensure_ascii=False))
        check("设备页 DOM 已追平实查数据", settled.get("ok") is True,
              f"try={settled.get('try')} got={settled.get('got')}")
        live = cdp(PROBE)
        print("  实查后卡片:", live.get("cards"))
        # 判据是「卡片上的文字 == 解析器给出的名字」，**不是**「等于某个写死的串」——
        # 配置里有没有别名是用户自己的状态，写死必然在某些机器上恒红。
        check("实查后设备页卡片与解析器一致（别名存在时显示别名）",
              live.get("deviceRaw") in live.get("cards", []),
              f"卡片={live.get('cards')}，解析器={live.get('deviceRaw')!r}")

        # ── 0. 清空两种形态 ──
        cdp(rename(SHORT, ""))
        cdp(rename(LONG, ""))
        base = cdp(PROBE)
        print("  清空后:", json.dumps(base, ensure_ascii=False))
        check("前置状态干净（map 为空）", base.get("map") == {}, f"map={base.get('map')}")
        check("设备页卡片显示原名", SHORT in base.get("cards", []),
              f"cards={base.get('cards')}")

        # ── 1. 从「音量页形态」改名 ⇒ 设备页 DOM 必须跟着变（本轮的核心判据）──
        cdp(rename(LONG, ALIAS))
        r = cdp(PROBE)
        print("  音量页改名后:", json.dumps(r, ensure_ascii=False))
        check("⛔ 设备信息页**已渲染的卡片标题**变成别名（反向同步）",
              ALIAS in r.get("cards", []), f"cards={r.get('cards')}")
        check("音量页卡片显示别名",
              any(ALIAS in x for x in r.get("audio", [])), f"audio={r.get('audio')}")
        check("后端两种形态都落了别名",
              r.get("map", {}).get(LONG) == ALIAS and r.get("map", {}).get(SHORT) == ALIAS,
              f"map={r.get('map')}")

        # ── 2. 从「音量页形态」点恢复默认 ⇒ 两边各自回原名 ──
        cdp(rename(LONG, ""))
        b = cdp(PROBE)
        print("  恢复默认后:", json.dumps(b, ensure_ascii=False))
        check("两种形态的键都删净（不是被遮蔽，是真删）", b.get("map") == {}, f"map={b.get('map')}")
        check("设备信息页卡片回到自己的原名",
              any(SHORT in x for x in b.get("cards", []))
              and not any(ALIAS in x for x in b.get("cards", [])),
              f"cards={b.get('cards')}")
        check("音量页卡片回到自己的原名",
              not any(ALIAS in x for x in b.get("audio", [])), f"audio={b.get('audio')}")
        check("解析器回落到各自原串（设备页短名 / 音量页长名）",
              b.get("deviceRaw") == SHORT and b.get("audioRaw") == LONG,
              f"deviceRaw={b.get('deviceRaw')!r} audioRaw={b.get('audioRaw')!r}")

        # ── 3. 从「设备页形态」点恢复默认 ⇒ 同样两边归位 ──
        cdp(rename(SHORT, ALIAS))
        cdp(rename(SHORT, ""))
        c = cdp(PROBE)
        print("  设备页恢复默认后:", json.dumps(c, ensure_ascii=False))
        check("从设备页恢复默认也删净两种形态", c.get("map") == {}, f"map={c.get('map')}")
        check("从设备页恢复默认后音量页也归位",
              not any(ALIAS in x for x in c.get("audio", [])), f"audio={c.get('audio')}")

        ok = sum(1 for _, o, _ in RESULTS if o)
        print(f"\n=== {ok}/{len(RESULTS)} 通过 ===")
        return 0 if ok == len(RESULTS) else 2
    finally:
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
        if os.path.exists(CFG_BAK):
            open(CFG, "w", encoding="utf-8").write(open(CFG_BAK, encoding="utf-8").read())
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

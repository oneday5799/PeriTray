# -*- coding: utf-8 -*-
"""验收：改名后**所有**出现该设备的表面用同一个名字；默认命名时各表面用各自的原名。

用户口径（2026-09-28）：
  · 默认命名：设备信息页（弹出窗口 / 设置窗口）+ 任务栏 ⇒ `小爱音箱-9205`
              音量控制页（弹出窗口 / 设置窗口）+ 托盘右键音频设备 ⇒ `耳机 (小爱音箱-9205)`（未开简化）
  · 改名之后：**所有**表面都显示同一个名字（与任务栏一致）

⛔⛔ **为什么拆成两个进程跑**（实测踩出来的硬约束）：设置窗口打开时，**隐藏的弹出
   窗口页面发起的 `invoke` 永不返回**（渲染进程还活着：CDP `Runtime.evaluate`
   正常，但 `window.__TAURI__.core.invoke` 的 Promise 永不 settle，且 Rust 侧
   连 `[cmd] rename_device` 日志都没有 ⇒ 消息没派发到主线程）。
   ⚠️ 这与本缺陷无关，是「一个 webview 可见 + 另一个隐藏」的环境行为；
   但它会让「一个进程里既改名又读两个页面」的验收脚本**必然超时**。
   ⇒ 阶段 1 只开弹出窗口（改名 + 读弹出两页）；阶段 2 只开设置窗口
   （**用 config.toml 预置前置状态**，只读不写）。

⚠️ 能直接读到的只有前端 DOM。任务栏 tooltip / 托盘 tooltip / 托盘右键菜单 /
   低电量通知都在 Rust 侧（`resolved_display_name` / `resolve_device_name`），
   由 `cargo test` + 代码走查覆盖，本脚本不重复断言。
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
CFG_BAK = CFG + ".namebak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

SHORT = "小爱音箱-9205"


def bare(name):
    """剥掉卡片标题尾部的「(默认)」徽标 —— 它标记默认设备，**不属于设备名**。"""
    if not name:
        return name
    return re.sub(r"[(（]默认[)）]$", "", name.strip())
LONG = "耳机 (小爱音箱-9205)"
ALIAS = "统一验收名"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def cdp(page, expr, timeout=60, retries=1):
    last = None
    for _ in range(retries + 1):
        try:
            r = subprocess.run([NODE, CDP, page, expr], capture_output=True,
                               text=True, encoding="utf-8", errors="replace", timeout=timeout)
            if r.returncode != 0:
                raise RuntimeError((r.stderr or r.stdout).strip())
            return json.loads(r.stdout.strip())
        except Exception as e:  # noqa: BLE001
            last = e
            time.sleep(2)
    raise RuntimeError(str(last))


def port_9222_alive():
    """调试端口是否还有人在监听（= 上一个实例是否真的退干净了）。

    ⛔ 为什么不用 `tasklist` 数进程：Python 捕获 `tasklist` 的输出时**不按
    本机 locale 解码**（重定向场景下常是 UTF-16），`count("peritray.exe")`
    恒为 0 ⇒ 误判成「应用已消失」。这个坑让我把两次正常启动判成启动失败。
    ⇒ 改用「HTTP 端点 + Popen 句柄」两个与编码无关的判据。
    """
    import urllib.request
    try:
        with urllib.request.urlopen("http://127.0.0.1:9222/json", timeout=1) as r:
            return r.status == 200
    except Exception:  # noqa: BLE001 —— 连不上就是没人监听
        return False


def kill_all(tries=20):
    """反复杀到调试端口空出来为止（端口空 = 上一个实例真的把 CDP 让掉了）。"""
    import subprocess as sp
    for _ in range(tries):
        if not port_9222_alive():
            return
        sp.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)
    raise RuntimeError("9222 端口仍被占用：旧实例没退干净")


def launch(settings=False, page="popup.html"):
    """杀掉旧实例 → 等端口空出来 → 起新实例 → **等 IPC 真的通**。

    ⛔ 必做的等待：`taskkill` 返回后进程仍在收尾（WebView2 子进程未退、
    调试端口未释放），此时 `/json` 里可能还挂着**上一个进程的孤儿页面**——
    它能执行 JS（`1+1` 有结果），但 IPC 通道已随进程被杀而断裂，
    任何 `invoke` 永不 settle。**看着像代码坏了，其实是探到了僵尸页面。**
    """
    import subprocess as sp
    kill_all()
    time.sleep(1)
    env = dict(os.environ)
    if settings:
        env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = "--disable-gpu-sandbox --remote-debugging-port=9222"
    proc = sp.Popen([EXE], cwd=WD, env=env)
    for k in range(30):
        time.sleep(2)
        if proc.poll() is not None:
            raise RuntimeError(f"应用启动后退出，退出码 {proc.returncode}")
        try:
            cdp(page, "(async()=>{await window.__TAURI__.core.invoke('get_config');"
                      "return JSON.stringify('ok');})()", timeout=20, retries=0)
            print(f"  (IPC 就绪，轮询 {k + 1} 次)")
            return
        except Exception:  # noqa: BLE001
            continue
    raise RuntimeError("30 次轮询后 IPC 仍未就绪")


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


# ── 弹出窗口：按**稳定属性**定位那一张卡（改名后名字会变，按名字找必然抽空）──
POPUP_PROBE = ("(async()=>{const inv=window.__TAURI__.core.invoke;"
               "const devs=await inv('get_devices');"
               "const d=devs.find(x=>x.name==='%s');"
               "if(!d)return JSON.stringify({err:'NO_DEVICE'});"
               "const dc=document.querySelector('#device-list .card.device[data-device-id=\"'+"
               "CSS.escape(deviceKey(d))+'\"] .device-name');"
               "const auds=await inv('get_audio_devices');"
               "const a=auds.find(x=>x.name==='%s');"
               "const ac=a?document.querySelector('.card[data-device-id=\"'+"
               "CSS.escape(a.id)+'\"] .audio-device-name'):null;"
               "return JSON.stringify({dev:dc?dc.textContent:null,"
               "audio:ac?ac.textContent:null,"
               "coreOk:auds.every(d=>typeof d.core_name==='string'&&d.core_name.length>0),"
               "coreSample:auds.filter(d=>d.name.indexOf('小爱')>=0)"
               ".map(d=>d.name+' ⇒ '+d.core_name)});})()") % (SHORT, LONG)

RENAME = ("(async()=>{await window.__TAURI__.core.invoke("
          "'rename_device',{original:'%s',newName:'%s'});"
          "await new Promise(r=>setTimeout(r,1500));return JSON.stringify('ok');})()")


def set_simplify(on):
    # ⚠️⚠️ `update_config(base, new_config)` 走 `merge_config(c, base, new_config)`，
    #    只应用**两者之间的差异** ⇒ 把同一个对象同时当 base 和 newConfig 传
    #    等于**空补丁、什么都不改**（实测：前置状态没生效，判据全红）。
    #    正确姿势：base = 改之前，newConfig = 改之后。
    return ("(async()=>{const inv=window.__TAURI__.core.invoke;"
            "const base=await inv('get_config');"
            "const next=JSON.parse(JSON.stringify(base));"
            "next.simplify_device_names=%s;"
            "await inv('update_config',{base:base,newConfig:next});"
            "await new Promise(r=>setTimeout(r,2000));"
            "const after=await inv('get_config');"
            "return JSON.stringify({applied:after.simplify_device_names});})()"
            ) % ("true" if on else "false")


# ── 设置窗口：只读。把整页 `.card-item-name` 抓回来，由 Python 判是否含期望串 ──
SETTINGS_PROBE = ("(async()=>{const grab=async(tab)=>{"
                  "document.querySelector('.win-nav-item[data-tab=\"'+tab+'\"]').click();"
                  "await new Promise(r=>setTimeout(r,1400));"
                  "return [...document.querySelectorAll('#tab-'+tab+' .card-item-name')]"
                  ".map(e=>e.textContent).filter(Boolean);};"
                  "return JSON.stringify({dev:await grab('device'),"
                  "aud:await grab('audio')});})()")


def seed_config(simplify, names):
    """直接改 config.toml 作为**前置状态**（应用启动时读入），避免跨页面 invoke。"""
    s = io_read(CFG)
    # ⚠️ TOML 布尔必须**小写**：Python 的 str(False) 会写出 `False`，
    #    应用解析失败会**整体回落默认值** ⇒ 前置状态根本没生效（实测踩过）。
    s = re.sub(r"^simplify_device_names = .*$",
               "simplify_device_names = %s" % ("true" if simplify else "false"),
               s, flags=re.M)
    block = "[device_names]\n" + "".join('"%s" = "%s"\n' % (k, v) for k, v in names.items())
    if "[device_names]" in s:
        s = re.sub(r"\[device_names\]\n(?:.*\n)*?(?=\n\[|\Z)", block, s, count=1)
    else:
        s = s.rstrip() + "\n\n" + block
    io_write(CFG, s)


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def io_write(p, s):
    import io
    io.open(p, "w", encoding="utf-8", newline="").write(s)


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml")
        return 1
    io_write(CFG_BAK, io_read(CFG))
    try:
        # ══ 阶段 1：弹出窗口两页（改名 → 读 → 恢复）══
        print("\n[阶段 1] 弹出窗口（设备信息页 + 音量控制页）")
        launch(settings=False)
        cdp("popup.html", RENAME % (SHORT, ""))
        cdp("popup.html", RENAME % (LONG, ""))
        cdp("popup.html", set_simplify(False))

        a = cdp("popup.html", POPUP_PROBE)
        print("  A 默认命名:", json.dumps(a, ensure_ascii=False))
        check("默认命名：设备信息页显示物理名", SHORT in (a.get("dev") or ""),
              f"dev={a.get('dev')!r}")
        check("默认命名：音量控制页显示音频端点原名", LONG in bare(a.get("audio")),
              f"audio={a.get('audio')!r}")
        check("后端随设备下发了 core_name（前端不再自己推短名）",
              a.get("coreOk") is True, f"{a.get('coreSample')}")
        check("默认命名：两侧**确实不同**（这是需求，不是 bug）",
              a.get("dev") != a.get("audio"), f"{a.get('dev')!r} vs {a.get('audio')!r}")

        cdp("popup.html", RENAME % (SHORT, ALIAS))
        b = cdp("popup.html", POPUP_PROBE)
        print("  B 改名后:", json.dumps(b, ensure_ascii=False))
        check("改名后：设备信息页 = 别名", ALIAS in (b.get("dev") or ""),
              f"dev={b.get('dev')!r}")
        check("改名后：音量控制页 = **同一个**别名", ALIAS in (b.get("audio") or ""),
              f"audio={b.get('audio')!r}")
        check("改名后：两页显示完全一致（剥掉「(默认)」徽标后）",
              bare(b.get("dev")) == bare(b.get("audio")),
              f"{b.get('dev')!r} / {b.get('audio')!r}")

        cdp("popup.html", RENAME % (SHORT, ""))
        c = cdp("popup.html", POPUP_PROBE)
        print("  C 恢复默认:", json.dumps(c, ensure_ascii=False))
        check("恢复默认：设备信息页回物理名", SHORT in (c.get("dev") or ""),
              f"dev={c.get('dev')!r}")
        check("恢复默认：音量控制页回端点原名", LONG in bare(c.get("audio")),
              f"audio={c.get('audio')!r}")

        cdp("popup.html", set_simplify(True))
        d = cdp("popup.html", POPUP_PROBE)
        print("  D 开简化:", json.dumps(d, ensure_ascii=False))
        check("开简化：音量页显示括号内短名，设备页不受影响",
              SHORT in (d.get("audio") or "") and LONG not in (d.get("audio") or "")
              and SHORT in (d.get("dev") or ""),
              f"audio={d.get('audio')!r} dev={d.get('dev')!r}")
        kill()

        # ══ 阶段 2：设置窗口两区（前置状态用 config.toml 预置，只读）══
        print("\n[阶段 2] 设置窗口（设备信息区 + 音量区）")
        seed_config(False, {})                       # 默认命名 + 关简化
        launch(settings=True, page='settings.html')
        s_a = cdp("settings.html", SETTINGS_PROBE)
        print("  A 默认命名:", json.dumps(s_a, ensure_ascii=False))
        check("默认命名：设置页设备信息区显示物理名",
              any(SHORT in x for x in s_a.get("dev", [])), f"dev={s_a.get('dev')}")
        check("默认命名：设置页音量区显示端点原名",
              any(LONG in x for x in s_a.get("aud", [])), f"aud={s_a.get('aud')}")
        kill()

        seed_config(False, {SHORT: ALIAS, LONG: ALIAS})   # 改名后的形态
        launch(settings=True, page='settings.html')
        s_b = cdp("settings.html", SETTINGS_PROBE)
        print("  B 改名后:", json.dumps(s_b, ensure_ascii=False))
        check("改名后：设置页设备信息区 = 别名",
              any(ALIAS in x for x in s_b.get("dev", [])), f"dev={s_b.get('dev')}")
        check("改名后：设置页音量区 = **同一个**别名",
              any(ALIAS in x for x in s_b.get("aud", [])), f"aud={s_b.get('aud')}")
        hit_dev = [x for x in s_b.get("dev", []) if ALIAS in x]
        hit_aud = [bare(x) for x in s_b.get("aud", []) if ALIAS in x]
        check("改名后：两区命中的条目逐字相同",
              bool(hit_dev) and bool(hit_aud) and hit_dev[0] == hit_aud[0],
              f"{hit_dev} / {hit_aud}")
        kill()

        # ── 场景 E：配置里存在**空白别名** ⇒ 两端都必须回落原名（A）──
        # 背景：`resolve_device_name_in` 曾对 `get()` 命中的空串直接 `return ""`，
        # 而 JS 的 `if (custom)` 判假回落 ⇒ 任务栏/托盘显示空白、页面显示原名。
        # 这里同时验两侧：任务栏列表走后端（Rust），设备/音量区走前端（JS）。
        seed_config(False, {SHORT: "", LONG: "   "})
        launch(settings=True, page='settings.html')
        e = cdp("settings.html", SETTINGS_PROBE)
        print("  E 空白别名:", json.dumps(e, ensure_ascii=False))
        check("空白别名：设置页设备信息区回落原名（不得空白）",
              any(SHORT in x for x in e.get("dev", [])), f"dev={e.get('dev')}")
        check("空白别名：设置页音量区回落端点原名（不得空白）",
              any(LONG in x for x in e.get("aud", [])), f"aud={e.get('aud')}")
        blank = [x for x in e.get("dev", []) + e.get("aud", []) if not x.strip()]
        check("空白别名：没有任何一处渲染成空串", not blank, f"空项={blank}")
        kill()

        ok = sum(1 for _, o, _ in RESULTS if o)
        print(f"\n=== {ok}/{len(RESULTS)} 通过 ===")
        return 0 if ok == len(RESULTS) else 2
    finally:
        # ⚠️ 顺序要紧：**先杀应用再还原配置**。反过来的话，运行中的实例内存里
        #    仍是被本脚本改过的值（simplify=false），任何一次落盘都会把
        #    false 重新写回文件 ⇒ 用户的设置被悄悄改掉（这个坑我踩了两次：
        #    下一个脚本因「音量页显示端点原名」而误报失败）。
        kill()
        time.sleep(1)
        if os.path.exists(CFG_BAK):
            io_write(CFG, io_read(CFG_BAK))
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

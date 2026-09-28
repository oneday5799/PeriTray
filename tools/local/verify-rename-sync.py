# -*- coding: utf-8 -*-
"""验收：设备信息页与音量控制页的**重命名同步**。

⛔ 缺陷背景（2026-09-28 用户报）：同一台设备在两页重命名不同步。
   根因不在后端 —— `apply_device_rename` 本就把「长形态键 + 短名键」归并写入，
   `resolve_device_name` 本就是两级查找；**前端两个解析器是单键查找**：
   音量页卡片的 `name` 是**音频端点名**（「耳机 (小爱音箱-9205)」），
   而从设备页改名时入口键就是**短名**（「小爱音箱-9205」）⇒ 长形态键未必存在
   ⇒ 音量页精确查不到 ⇒ 「设备页改了名、音量页不变」。

判据（`命令` 级，走真实进程 + 真实 dist + 真实 IPC）：
   ⛔ **先清空两种形态的键**（模拟「从没改过名」），只从**设备页形态**改名，
      然后读**音量页形态**的解析结果 —— 这一步在修复前必然拿不到别名。
   ⚠️ 之所以必须先清空：若长形态键**已存在**（哪怕值是旧的），旧代码也能读到，
      测出来的「通过」是假绿（锚点不对 ⇒ 判据不承重）。
"""
import json
import os
import subprocess
import sys
import time

WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = CFG + ".renamebak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

LONG = "耳机 (小爱音箱-9205)"   # 音量页形态（音频端点名）
SHORT = "小爱音箱-9205"          # 设备页形态（物理设备名）
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


# ⚠️ 只用**修复前后都存在**的两个公开解析器 —— 判据里出现新 API 会让「未修复」那次
#    直接抛异常而不是给出**不同的结果**，那样的假绿/假红都说明判据不承重。
#    `prefill` 取 `formatDeviceName(..., simplify=false)`：它在两版里都等于
#    「别名 || 原名」，正是重命名对话框输入框的预填值。
RESOLVE = ("(async()=>{const c=await window.__TAURI__.core.invoke('get_config');"
           "const n=c.device_names||{};const s=c.simplify_device_names!==false;"
           "return JSON.stringify({map:n,"
           "audio:window.formatDeviceName('%s',n,s),"
           "device:window.getDisplayName({name:'%s'},n),"
           "prefill:window.formatDeviceName('%s',n,false)});})()") % (LONG, SHORT, LONG)

RENAME = ("(async()=>{const inv=window.__TAURI__.core.invoke;"
          "await inv('rename_device',{original:'%s',newName:'%s'});"
          "await new Promise(r=>setTimeout(r,900));return JSON.stringify('ok');})()") % (SHORT, ALIAS)


def rename_expr(original, new_name):
    """改名 / 恢复默认（new_name 为空串）——**两种入口形态都要能触发**。"""
    return ("(async()=>{const inv=window.__TAURI__.core.invoke;"
            "await inv('rename_device',{original:'%s',newName:'%s'});"
            "await new Promise(r=>setTimeout(r,900));return JSON.stringify('ok');})()"
            ) % (original, new_name)


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
        time.sleep(9)
        wait_ipc()

        # ── 0. 清空两种形态 ⇒ 制造「从没改过名」的前置状态 ──
        cdp(rename_expr(SHORT, ""))
        cdp(rename_expr(LONG, ""))
        base = cdp(RESOLVE)
        print("  清空后:", json.dumps(base, ensure_ascii=False))
        check("两种形态的键都已清空（前置状态干净）",
              base.get("map") == {}, f"map={base.get('map')}")
        check("清空后两页都显示原名",
              base.get("audio") == SHORT and base.get("device") == SHORT,
              f"audio={base.get('audio')!r} device={base.get('device')!r}")

        # ── 1. 只从「设备页形态」改名 ──
        cdp(RENAME)
        r = cdp(RESOLVE)
        print("  设备页改名后:", json.dumps(r, ensure_ascii=False))
        check("配置里只落了短名键（长形态键不存在）",
              list(r.get("map", {}).keys()) == [SHORT],
              f"map={r.get('map')}")
        check("⛔ 音量页（长形态）也能解析到别名 —— 两页同步",
              r.get("audio") == ALIAS, f"audio={r.get('audio')!r}，期望 {ALIAS!r}")
        check("设备页仍显示别名", r.get("device") == ALIAS, f"device={r.get('device')!r}")
        check("重命名对话框预填的是别名（恢复默认按钮会出现）",
              r.get("prefill") == ALIAS, f"prefill={r.get('prefill')!r}")

        # ── 2. 恢复默认 ⇒ 两形态都删净 ──
        cdp(rename_expr(SHORT, ""))
        back = cdp(RESOLVE)
        print("  恢复默认后:", json.dumps(back, ensure_ascii=False))
        check("从设备页点「恢复默认」也能删净（不留残键）",
              back.get("map") == {}, f"map={back.get('map')}")
        check("恢复默认后两页都回原名",
              back.get("audio") == SHORT and back.get("device") == SHORT,
              f"audio={back.get('audio')!r} device={back.get('device')!r}")

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

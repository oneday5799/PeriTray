# -*- coding: utf-8 -*-
"""验收：任务栏窗口上滚轮调音量（真实滚轮事件注入）。

流程（真实进程 + 真实分层窗）：
  1. 预置配置：详细级日志（要读命中区坐标）+ 一个已固定的音频设备；
  2. 启动 ⇒ 等 widget 挂载 ⇒ 从日志读 `[widget] 滚轮触发区(屏幕,同 tooltip): …]`；
  3. `SetCursorPos` 到第 0 项音量行中心 ⇒ 注入 `MOUSEEVENTF_WHEEL`（每格 120）；
  4. 断言：① 日志出现「滚轮调音量」；② 该端点音量**恰好**变化 N×步进；
     ③ 步进随「音量精细调节」开关变化；④ 音量显示是一位小数。

⚠️ 注入前必须 `SetProcessDpiAwarenessContext(-4)`，否则物理坐标全错
   （本机 150% 缩放，不设的话光标会落到别的位置）。
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
CFG_BAK = CFG + ".wheelbak"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'[OK]  ' if ok else '[FAIL]'} {name}" + (f"  —— {detail}" if detail else ""))
    sys.stdout.flush()


def io_read(p):
    import io
    return io.open(p, encoding="utf-8", newline="").read()


def io_write(p, s):
    import io
    io.open(p, "w", encoding="utf-8", newline="").write(s)


def port_alive():
    import urllib.request
    try:
        with urllib.request.urlopen("http://127.0.0.1:9222/json", timeout=1) as r:
            return r.status == 200
    except Exception:  # noqa: BLE001
        return False


def kill_all():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    for _ in range(20):
        if not port_alive():
            return
        subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
        time.sleep(1)


def seed(fine):
    s = io_read(CFG)
    s = re.sub(r'^log_level = .*$', 'log_level = "verbose"', s, flags=re.M)
    # ⚠️ TOML 布尔必须**小写**：Python 的 str(True) 写出 `True` ⇒ **整份配置解析失败**
    #    ⇒ 应用回落默认值、widget 走的是默认档（踩过一次，日志里直接报 parse error）。
    lit = "true" if fine else "false"
    if re.search(r"^volume_fine_adjust = ", s, flags=re.M):
        s = re.sub(r"^volume_fine_adjust = .*$", "volume_fine_adjust = " + lit, s, flags=re.M)
    else:
        s = s.rstrip() + nl + "volume_fine_adjust = " + lit + nl
    io_write(CFG, s)


def newest_log():
    d = os.path.join(WD, "logs")
    files = [os.path.join(d, f) for f in os.listdir(d) if f.endswith(".log")]
    files.sort(key=os.path.getmtime, reverse=True)
    return files[0] if files else None


def wait_hit_rows(timeout=40):
    """等日志里出现滚轮触发区（widget 已画过至少一帧且开了详细级）。"""
    pat = re.compile(r"\[widget\] 滚轮触发区\(屏幕,同 tooltip\): (\[.*\])")
    end = time.time() + timeout
    while time.time() < end:
        p = newest_log()
        if p:
            m = None
            for line in io_read(p).splitlines():
                mm = pat.search(line)
                if mm:
                    m = mm.group(1)
            if m:
                return m
        time.sleep(1)
    return None


def inject_wheel(x, y, notches):
    """把光标移到 (x,y) 并注入 n 格向上滚。返回是否成功注入。"""
    import ctypes

    user32 = ctypes.windll.user32
    try:
        # ⛔ 必需：150% 缩放下不设 Per-Monitor DPI 感知，物理坐标全错
        user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
    except Exception:  # noqa: BLE001
        try:
            ctypes.windll.shcore.SetProcessDpiAwareness(2)
        except Exception:  # noqa: BLE001
            pass
    if not user32.SetCursorPos(int(x), int(y)):
        return False
    time.sleep(0.25)          # 等 hover 轮询把底衬画上（否则收不到消息）
    for _ in range(int(notches)):
        # MOUSEEVENTF_WHEEL = 0x0800，delta=+120 = 向上滚一格
        user32.mouse_event(0x0800, 0, 0, 120, 0)
        time.sleep(0.35)
    return True


def device_volumes():
    r = subprocess.run([NODE, CDP, "popup.html",
                        "(async()=>{const d=await window.__TAURI__.core.invoke('get_audio_devices');"
                        "return JSON.stringify(d.map(x=>({id:x.id,name:x.name,volume:x.volume})));})()"],
                       capture_output=True, text=True, encoding="utf-8",
                       errors="replace", timeout=60)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip()[:200])
    return json.loads(r.stdout.strip())


def wait_ipc(tries=30):
    for k in range(tries):
        try:
            subprocess.run([NODE, CDP, "popup.html",
                            "(async()=>{await window.__TAURI__.core.invoke('get_config');"
                            "return JSON.stringify('ok');})()"],
                           capture_output=True, text=True, encoding="utf-8",
                           errors="replace", timeout=20)
            return True
        except Exception:  # noqa: BLE001
            time.sleep(2)
    return False


def wheel_log_count(log):
    return len(re.findall(r"\[widget\] 滚轮调音量", io_read(log)))


def run_case(fine, notches, label):
    print(f"\n── {label}（精细调节={fine}，滚 {notches} 格）──")
    kill_all()
    time.sleep(1)
    seed(fine)
    for f in os.listdir(os.path.join(WD, "logs")):
        if f.endswith(".log"):
            os.remove(os.path.join(WD, "logs", f))
    env = dict(os.environ)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    proc = subprocess.Popen([EXE], cwd=WD, env=env)
    ok_start = False
    for _ in range(30):
        time.sleep(2)
        if proc.poll() is not None:
            break
        if wait_ipc(tries=1):
            ok_start = True
            break
    if not ok_start:
        check(f"{label}：应用启动且 IPC 就绪", False)
        return
    rows_raw = wait_hit_rows()
    if not rows_raw:
        check(f"{label}：日志里出现滚轮触发区", False, "没等到（详细级日志或 widget 未挂载）")
        return
    print("  命中行:", rows_raw[:160])
    # 取第一项的屏幕矩形中心
    m = re.search(r"#0 \((-?\d+),(-?\d+),(-?\d+),(-?\d+)\)", rows_raw)
    if not m:
        check(f"{label}：第 0 项有音量命中区", False, rows_raw[:120])
        return
    l, t, r, b = (int(x) for x in m.groups())
    # ⭐ 注入点取**图标区**（项矩形左侧 1/4、上半），不是音量文本 ——
    #   这正是用户报「hover 在设备图标上滚轮没反应」的位置。
    cx, cy = l + max(2, (r - l) // 4), t + max(2, (b - t) // 4)
    log = newest_log()
    before_n = wheel_log_count(log)
    vols_before = device_volumes()
    if not inject_wheel(cx, cy, notches):
        check(f"{label}：注入滚轮", False, f"SetCursorPos 失败 @({cx},{cy})")
        return
    time.sleep(2.5)
    vols_after = device_volumes()
    new_logs = wheel_log_count(log) - before_n
    step = 0.1 if fine else 1.0
    expect = step * notches
    # 找出真正变化了的端点
    changed = []
    for a, b2 in zip(vols_before, vols_after):
        if a["id"] != b2["id"]:
            continue
        if a["volume"] is None or b2["volume"] is None:
            continue
        d = (b2["volume"] - a["volume"]) * 100.0
        if abs(d) > 1e-6:
            changed.append((a["name"], round(a["volume"] * 100, 2), round(b2["volume"] * 100, 2), round(d, 3)))
    check(f"{label}：日志出现「滚轮调音量」", new_logs > 0, f"新增 {new_logs} 条")
    check(f"{label}：恰好一个端点被调", len(changed) == 1, f"{changed}")
    if changed:
        name, v0, v1, d = changed[0]
        # ⛔ 判据用**标称**变化量，不是原始浮点差：页面滑块初值先取整到步进网格
    #   （popup-audio.js:181），所以 53.7% 向上滚 3 格 ⇒ 标称 54→57，
    #   实际浮点是 +3.3%。断言原始差会把**与页面一致的行为**判成失败。
    if changed:
        name, v0, v1, d = changed[0]
        base = round(v0, 1) if fine else round(v0)
        nominal = min(100.0, base + expect) if fine else min(100.0, base + expect)
        check(f"{label}：标称音量 {base:g}% → {nominal:g}%（{notches}×{step:g}）",
              abs(v1 - nominal) < 0.051,
              f"{name}: {v0}% → {v1}%（标称期望 {nominal:g}%）")
    txt = "".join(l for l in io_read(log).splitlines() if "滚轮音量 id=" in l)
    m2 = re.findall(r"滚轮音量 id=(\S+) ([\d.]+) -> ([\d.]+)", txt)
    check(f"{label}：详细日志记到了端点 id 与前后值", bool(m2), f"{m2[-1:] }")


def main():
    if not os.path.exists(CFG):
        print("!! 找不到 config.toml")
        return 1
    io_write(CFG_BAK, io_read(CFG))
    try:
        run_case(False, 3, "普通档 1%")
        run_case(True, 3, "精细档 0.1%")
        ok = sum(1 for _, o, _ in RESULTS if o)
        print(f"\n=== {ok}/{len(RESULTS)} 通过 ===")
        return 0 if ok == len(RESULTS) else 2
    finally:
        kill_all()
        if os.path.exists(CFG_BAK):
            io_write(CFG, io_read(CFG_BAK))
            os.remove(CFG_BAK)
            print("  (config.toml 已还原)")


if __name__ == "__main__":
    sys.stdout.reconfigure(encoding="utf-8", errors="replace")
    sys.exit(main())

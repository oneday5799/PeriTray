"""任务栏 widget **拖拽**的真机验收（B3-A 里程碑 4）。

⛔⛔ **为什么不用真实鼠标注入**（2026-09-25 本机实测，别再重复探索）：
   本机（Win11 26100，session 1、窗口站 `WinSta0`、`SM_MOUSEPRESENT=1`）**无法注入鼠标**：
     · `SetCursorPos`         → 返回 **0**，`GetLastError` = 0，光标纹丝不动
     · `SetPhysicalCursorPos` → 返回 **0**
     · `SendInput`（绝对移动）→ 返回 **1（自称成功）**，光标仍纹丝不动
     · `mouse_event`           → 无错误，光标仍纹丝不动
   关掉沙箱后同样如此 ⇒ 是**会话/令牌层面**的限制，不是本工具链的问题。
   ⇒ 验收改为：**直接 `PostMessage` 三条鼠标消息**，由临时注入把「光标 x」的来源
     从 `GetCursorPos()` 换成消息 `lParam`。跑的是**真实的窗口过程 + 真实的
     `SetWindowPos` + 真实的配置落盘**，只把「光标位置」这一个环境拿不到的量做了替换。

判据（每条都可证伪）：
  P. **前提**：窗口存在 **且** 底衬的可观测后果与 `locked` 相符。
     ⛔ 为什么用**行为**而不是解析日志里的 `locked=`：
        · 上一版把 `taskbar_position_locked = false` 追加到 config.toml **末尾**，
          而末尾落在 `[device_shortcuts."..."]` 表里 ⇒ TOML 裸键属于最近一个表头 ⇒
          **静默失效** ⇒ 应用一直用默认 `true` ⇒ 5 条判据假红；
        · 改用「按字符偏移切日志」后，日志文件切换会让切片整段落空 ⇒ 又假红。
        ⇒ 最稳的前提断言是**功能自身的可观测后果**：底衬在不在，正是 `locked` 的后果。
  A. **底衬让整块 widget 可命中**：逐 2px 采样命中率应从 11/68 升到接近满格。
  B. **能拖动**：三条消息驱动后窗口位移 ≈ 光标位移（±3px）。
  C. **落位落盘**：`taskbar_custom_x` == 实测「父窗客户区 x」。
  D. **重启保持**：重启后窗口回到 E 记下的落点（取**有辨识度的中间值**，
     避免与 `LAST_X` 的哨兵 0、或与贴靠结果撞车而失去区分力）。
  E. **钳制生效**：往两端之外猛拖，窗口不越出任务栏，且贴住边界。
  F. **固定位置时不可拖**：`locked=true` 时拖拽无效、且不铺底衬。
"""

import ctypes
import ctypes.wintypes as w
import glob
import hashlib
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

SRC = Path(r"D:\Code\PeriTray\src-tauri\src\taskbar_widget.rs")
WD = r"D:\Code\PeriTray\src-tauri\target\debug"
EXE = os.path.join(WD, "PeriTray.exe")
CFG = os.path.join(WD, "config.toml")
CFG_BAK = os.path.join(WD, "config.toml.dragbak")
CARGO = r"C:\Users\Oneday\.cargo\bin\cargo.exe"
NODE = r"C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe"
CDP = r"D:\Code\PeriTray\tools\cdp-eval.mjs"

# ── 临时注入：把「光标 x」的来源换成消息 lParam（带 nonce 隔离真实鼠标）──
#
# ⛔⛔ 为什么要 nonce：拖拽期间 `SetCapture` 生效 ⇒ **真实鼠标的任何移动**
#   都会变成投递到 widget 的 `WM_MOUSEMOVE`。本机是**有人在用的活会话**，
#    实测因此把窗口拽到别处（`拖拽落位 rel_x=784`，而期望 1292）。
#    ⇒ 测试消息用 `MAKELPARAM(x, 0x7FFF)` 打标（真实消息的客户区 y 不可能是 32767），
#      注入里只认带标的消息，其余一律丢弃 —— 让验收对「活人用鼠标」免疫。
NONCE_Y = 0x7FFF
INJ = [
    (
        "注入 DRAG_INJECT_X / DRAG_INJECT_OK 静态量",
        "static DRAG_ACTIVE: std::sync::atomic::AtomicBool = "
        "std::sync::atomic::AtomicBool::new(false);",
        "static DRAG_ACTIVE: std::sync::atomic::AtomicBool = "
        "std::sync::atomic::AtomicBool::new(false);\n"
        "#[cfg(target_os = \"windows\")]\n"
        "static DRAG_INJECT_X: std::sync::atomic::AtomicI32 = "
        "std::sync::atomic::AtomicI32::new(0);\n"
        "#[cfg(target_os = \"windows\")]\n"
        "static DRAG_INJECT_OK: std::sync::atomic::AtomicBool = "
        "std::sync::atomic::AtomicBool::new(false);",
    ),
    (
        "wnd_proc：只接受带 nonce 的鼠标消息（隔离真实鼠标）",
        "        if msg == WM_LBUTTONDOWN {\n"
        "            if super::drag_begin(hwnd) {",
        "        if msg == WM_LBUTTONDOWN || msg == WM_MOUSEMOVE {\n"
        "            let nonce = ((lp as usize >> 16) & 0xFFFF) == 0x7FFF;\n"
        "            super::DRAG_INJECT_OK.store(\n"
        "                nonce,\n"
        "                std::sync::atomic::Ordering::Release,\n"
        "            );\n"
        "            if nonce {\n"
        "                super::DRAG_INJECT_X.store(\n"
        "                    ((lp as u16) as i16) as i32,\n"
        "                    std::sync::atomic::Ordering::Release,\n"
        "                );\n"
        "            } else {\n"
        "                return 0;\n"
        "            }\n"
        "        }\n"
        "        if msg == WM_LBUTTONDOWN {\n"
        "            if super::drag_begin(hwnd) {",
    ),
    (
        "drag_begin：带标时用注入值，否则走真实 GetCursorPos",
        "    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };\n"
        "    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {\n"
        "        return false;\n"
        "    }",
        "    let pt = if DRAG_INJECT_OK.load(Ordering::Acquire) {\n"
        "        windows_sys::Win32::Foundation::POINT {\n"
        "            x: DRAG_INJECT_X.load(Ordering::Acquire),\n"
        "            y: 0,\n"
        "        }\n"
        "    } else {\n"
        "        let mut p = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };\n"
        "        if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut p) } == 0 {\n"
        "            return false;\n"
        "        }\n"
        "        p\n"
        "    };",
    ),
    (
        "drag_move：用注入值（无标消息已在 wnd_proc 丢弃）",
        "    let mut pt = windows_sys::Win32::Foundation::POINT { x: 0, y: 0 };\n"
        "    if unsafe { windows_sys::Win32::UI::WindowsAndMessaging::GetCursorPos(&mut pt) } == 0 {\n"
        "        return;\n"
        "    }",
        "    let pt = windows_sys::Win32::Foundation::POINT {\n"
        "        x: DRAG_INJECT_X.load(Ordering::Acquire),\n"
        "        y: 0,\n"
        "    };",
    ),
]

u = ctypes.windll.user32
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    u.SetProcessDPIAware()

WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE = 0x0201, 0x0202, 0x0200
MK_LBUTTON = 0x0001
RESULTS = []


def check(name, ok, detail=""):
    RESULTS.append((name, ok, detail))
    print(f"  {'✅' if ok else '❌'} {name}" + (f"  —— {detail}" if detail else ""))


# ── 基础 ──────────────────────────────────────────────────────────

def log_text():
    logs = sorted(glob.glob(os.path.join(WD, "logs", "*.log")), key=os.path.getmtime)
    return open(logs[-1], "r", encoding="utf-8", errors="replace").read() if logs else ""


def widget_hwnd():
    """直接按**类名**在任务栏下查 widget。

    ⛔ 不解析日志里的 `mount report: hwnd=`：那串只在 `spawn_widget` 路径打，
       窗口也可能经 `apply_from_config` 挂载 ⇒ 会拿到**上一次实例的失效句柄**
       （实测表现为「日志里明明有 定位: 行，却 IsWindow()==0」）。
    """
    tray = u.FindWindowW("Shell_TrayWnd", None)
    if not tray:
        return 0
    return u.FindWindowExW(tray, None, "PeriTrayTaskbarWidget", None)


def widget_rect(hwnd):
    r = w.RECT()
    u.GetWindowRect(hwnd, ctypes.byref(r))
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def taskbar_rect():
    r = w.RECT()
    u.GetWindowRect(u.FindWindowW("Shell_TrayWnd", None), ctypes.byref(r))
    return (r.left, r.top, r.right - r.left, r.bottom - r.top)


def client_origin(hwnd):
    pt = w.POINT(0, 0)
    u.ClientToScreen(u.GetParent(hwnd), ctypes.byref(pt))
    return (pt.x, pt.y)


def hittest(rect, step=2):
    left, top, width, height = rect
    y = top + height // 2
    hit = total = 0
    for dx in range(0, width, step):
        h = u.WindowFromPoint(w.POINT(left + dx, y))
        total += 1
        if h:
            cls = ctypes.create_unicode_buffer(256)
            u.GetClassNameW(h, cls, 256)
            if cls.value == "PeriTrayTaskbarWidget":
                hit += 1
    return hit, total


def cdp(expr, timeout=60):
    r = subprocess.run([NODE, CDP, "settings.html", expr],
                       capture_output=True, text=True, timeout=timeout)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


def kill():
    subprocess.run(["taskkill", "/F", "/IM", "PeriTray.exe"], capture_output=True)
    time.sleep(1)


def launch():
    env = dict(os.environ)
    env["PM_DEV_OPEN_SETTINGS"] = "1"
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = \
        "--disable-gpu-sandbox --remote-debugging-port=9222"
    return subprocess.Popen([EXE], cwd=WD, env=env)


# ── 配置读写 ──────────────────────────────────────────────────────

def drop_config_line(key):
    """删掉 `key` 这一项（**必须能处理多行数组**）。

    ⛔⛔ 只删 `key = ...` 那一行是不够的：应用把 `pinned_taskbar_devices` 写成
       ```
       pinned_taskbar_devices = [
           "c:...",
       ]
       ```
       ⇒ 留下数组元素与 `]` ⇒ **TOML 解析失败** ⇒ 应用回落到默认配置（无固定项）。
       随后 `cdp(PIN)` 会把「其实已固定」的项**取消固定** ⇒ 窗口被拆掉 ⇒
       表现为「日志里明明有 定位: 行，却找不到窗口」。
    """
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    pat = re.compile(rf"^{re.escape(key)}\s*=\s*\[", re.M)
    while True:
        m = pat.search(txt)
        if not m:
            break
        j = m.end() - 1          # 指向 '['
        depth = 0
        while j < len(txt):
            if txt[j] == "[":
                depth += 1
            elif txt[j] == "]":
                depth -= 1
                if depth == 0:
                    j += 1
                    break
            j += 1
        while j < len(txt) and txt[j] != "\n":
            j += 1
        if j < len(txt):
            j += 1
        txt = txt[:m.start()] + txt[j:]
    txt = re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M)
    open(CFG, "w", encoding="utf-8").write(txt)


def set_config_line(key, line):
    """把 `key` 写成 **config.toml 的顶层键**。

    ⛔⛔ 不能追加到文件末尾：TOML 里裸键属于**最近一个 `[table]` 头**。本仓 config.toml
       末尾是 `[device_shortcuts."{...}"]`，追加进去就变成那张表里的键 ⇒ **静默失效**。
    ⛔ 也不能用 `txt.find("[")`：会命中 `hidden_devices = []` 里的方括号，
       把键插进一行中间（实测直接把配置写坏）。
    """
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    txt = re.sub(rf"^{re.escape(key)}\s*=.*\n?", "", txt, flags=re.M)
    m = re.search(r"^\[", txt, re.M)
    txt = (txt.rstrip("\n") + "\n" + line + "\n") if m is None \
        else (txt[:m.start()] + line + "\n" + txt[m.start():])
    open(CFG, "w", encoding="utf-8").write(txt)


def config_value(key):
    txt = open(CFG, encoding="utf-8", errors="replace").read()
    m = re.search(r"^\[", txt, re.M)
    head = txt if m is None else txt[:m.start()]
    m2 = re.search(rf"^{re.escape(key)}\s*=\s*(.+)$", head, re.M)
    return m2.group(1).strip() if m2 else None


# ── 拖拽：直接投递消息 ────────────────────────────────────────────

def post_drag(hwnd, x0, x1, steps=6):
    """把 `x0 → x1` 的拖拽序列投递给 widget。

    `lParam` = `MAKELPARAM(光标屏幕 x, NONCE_Y)`：高字是**测试标记**，
    让注入端能区分「测试消息」与「真实鼠标消息」（见 `INJ` 注释）。
    """
    def lp(x):
        return ((NONCE_Y & 0xFFFF) << 16) | (x & 0xFFFF)

    ok = u.PostMessageW(hwnd, WM_LBUTTONDOWN, MK_LBUTTON, lp(x0))
    time.sleep(0.25)
    for i in range(1, steps + 1):
        x = x0 + (x1 - x0) * i // steps
        u.PostMessageW(hwnd, WM_MOUSEMOVE, MK_LBUTTON, lp(x))
        time.sleep(0.08)
    time.sleep(0.15)
    u.PostMessageW(hwnd, WM_LBUTTONUP, 0, lp(x1))
    time.sleep(0.6)
    return ok


# ⭐ 幂等勾选：只补**缺的**，绝不因为「已经勾了」而把它取消掉。
#    ⛔ 上一版直接 toggle，一旦 `drop_config_line` 没删干净（多行数组只删首行 ⇒
#       TOML 解析失败 ⇒ 应用回落默认），toggle 就变成「取消固定」⇒ 窗口被拆掉。
PIN = ("(async () => {"
       "  const inv = window.__TAURI__.core.invoke;"
       "  const devs = await inv('get_taskbar_devices');"
       "  const sel = await inv('get_selectable_devices');"
       "  const pinned = new Set((sel || []).filter(d => d.pinned).map(d => d.key));"
       "  const audio = (devs || []).find(d => d.audio_device_id && d.volume != null);"
       "  const plain = (devs || []).find(d => !d.audio_device_id && d.battery != null);"
       "  let pick = [audio, plain].filter(Boolean);"
       "  if (pick.length < 2) { pick = (devs || []).slice(0, 2); }"
       "  const added = [];"
       "  for (const d of pick) {"
       "    if (pinned.has(d.key)) continue;"
       "    await inv('toggle_pinned_taskbar_device',"
       "      { key: d.key, fallback: null, alias: null });"
       "    added.push(d.name);"
       "  }"
       "  return JSON.stringify({pick: pick.map(d => d.name), added}); })()")


def setup(locked, label, clear_custom=True):
    """写配置 → 启动 → 勾选设备 → **用行为断言前提**。返回 (proc, hwnd)。

    ⚠️ `clear_custom=False`：步骤 F 会再调一次本函数，若那时清掉 `taskbar_custom_x`，
       步骤 D（重启保持）就无从校验了。
    """
    print(f"\n=== 启动（{label}，locked={locked}）===")
    drop_config_line("pinned_taskbar_devices")
    if clear_custom:
        drop_config_line("taskbar_custom_x")
    set_config_line("taskbar_position_locked",
                    f"taskbar_position_locked = {'true' if locked else 'false'}")
    print(f"  写盘后顶层: locked = {config_value('taskbar_position_locked')}"
          f"，custom_x = {config_value('taskbar_custom_x')}")
    proc = launch()
    time.sleep(9)
    hwnd = 0
    for attempt in range(3):
        print(f"  pinned[{attempt}]:", cdp(PIN))
        for _ in range(20):
            hwnd = widget_hwnd()
            if hwnd:
                break
            time.sleep(0.5)
        if hwnd:
            break
        print(f"  ⚠️ 第 {attempt + 1} 次仍未出现 widget，重试勾选…")
    time.sleep(2)
    if not hwnd:
        check(f"P 前提成立（{label}）", False, "30s 内未出现 widget 窗口")
        return proc, 0
    # ⭐ 用**功能自身的可观测后果**断言前提：底衬在不在 = `locked` 有没有生效。
    hit, total = hittest(widget_rect(hwnd))
    rate = hit / max(1, total)
    if locked:
        check(f"P 前提成立（{label}）", rate < 0.3,
              f"窗口在、命中 {hit}/{total}（locked ⇒ 不铺底衬）")
    else:
        check(f"P 前提成立（{label}）", rate >= 0.9,
              f"窗口在、命中 {hit}/{total}（unlocked ⇒ 铺底衬）")
    return proc, hwnd


# ══ 主流程 ═══════════════════════════════════════════════════════

original = SRC.read_text(encoding="utf-8")
base = hashlib.sha256(original.encode()).hexdigest()[:16]
print(f"源文件 sha256[:16] = {base}")

patched = original
for desc, old, new in INJ:
    if patched.count(old) != 1:
        print(f"⛔ 注入锚点不唯一/未命中（{patched.count(old)} 处）：{desc}")
        sys.exit(2)
    patched = patched.replace(old, new, 1)
SRC.write_text(patched, encoding="utf-8")
print("已注入「光标 x 改由消息 lParam 提供」，构建中…")
r = subprocess.run([CARGO, "build"], cwd=str(SRC.parent.parent),
                   capture_output=True, text=True, encoding="utf-8", errors="replace")
errs = [ln for ln in ((r.stdout or "") + (r.stderr or "")).splitlines()
        if ln.startswith("error") or "error[" in ln]
if errs:
    print("⛔ 构建失败：\n" + "\n".join(errs[:15]))
    SRC.write_text(original, encoding="utf-8")
    sys.exit(2)


def restore():
    SRC.write_text(original, encoding="utf-8")
    now = hashlib.sha256(SRC.read_text(encoding="utf-8").encode()).hexdigest()[:16]
    print(f"还原后 sha256[:16] = {now}  {'✅ 一致' if now == base else '⛔ 不一致'}")
    print("重新构建以恢复正式产物…")
    subprocess.run([CARGO, "build"], cwd=str(SRC.parent.parent), capture_output=True)


kill()
shutil.copy2(CFG, CFG_BAK)

proc, hwnd = setup(locked=False, label="解锁：可拖拽")
mtime_before = os.path.getmtime(CFG)   # ⚠️ 必须在 setup 之后取：setup 自己也会写文件
if not hwnd:
    print("⛔ 找不到 widget —— 后续判据无法执行")
    proc.terminate(); time.sleep(1); kill()
    shutil.copy2(CFG_BAK, CFG); restore()
    sys.exit(2)

tb = taskbar_rect()
co = client_origin(hwnd)
r0 = widget_rect(hwnd)
print(f"\n任务栏 rect = {tb}   客户区原点 = {co}")
print(f"widget rect = {r0}   父窗客户区 x = {r0[0] - co[0]}")

# ── A ──
print("\n=== A. 命中测试（底衬是否让整块 widget 可点）===")
hit, total = hittest(r0)
print(f"  命中 {hit}/{total}（矩形宽 {r0[2]}）")
check("A 底衬命中率 ≥ 90%", hit >= total * 0.9, f"{hit}/{total}（无底衬时实测 11/68）")

# ── B ──
print("\n=== B. 拖拽位移（PostMessage 驱动）===")
CX0 = 1000
DX = 150
posted = post_drag(hwnd, CX0, CX0 + DX)
r1 = widget_rect(hwnd)
moved = r1[0] - r0[0]
print(f"  PostMessage ok={posted}；光标 x {CX0} → {CX0 + DX}（+{DX}）")
print(f"  窗口 left {r0[0]} → {r1[0]}（位移 {moved}）")
check("B 窗口跟随光标移动（±3px）", abs(moved - DX) <= 3, f"位移 {moved}, 期望 {DX}")

# ── C ──
print("\n=== C. 落位写进配置 ===")
time.sleep(1.5)
expect_rel = r1[0] - client_origin(hwnd)[0]
raw = config_value("taskbar_custom_x")
print(f"  期望 taskbar_custom_x = {expect_rel}，配置里 = {raw}")
check("C taskbar_custom_x == 父窗客户区 x", raw == str(expect_rel),
      f"配置 {raw} vs 实测 {expect_rel}")
check("C 配置文件确实被应用改写过",
      os.path.getmtime(CFG) > mtime_before,
      f"mtime {mtime_before:.1f} → {os.path.getmtime(CFG):.1f}")

# ── E ──
print("\n=== E. 越界钳制 ===")
post_drag(hwnd, CX0, CX0 + 3000)
r2 = widget_rect(hwnd)
tb3 = taskbar_rect()
print(f"  猛拖 +3000 ⇒ widget right = {r2[0] + r2[2]}, 任务栏 right = {tb3[0] + tb3[2]}")
check("E 右缘不越出任务栏", r2[0] + r2[2] <= tb3[0] + tb3[2],
      f"{r2[0] + r2[2]} vs {tb3[0] + tb3[2]}")
check("E 右缘贴住任务栏右缘", (tb3[0] + tb3[2]) - (r2[0] + r2[2]) <= 2,
      f"差 {tb3[0] + tb3[2] - (r2[0] + r2[2])}px")

post_drag(hwnd, CX0 + 3000, CX0 - 3000)
r3 = widget_rect(hwnd)
tb4 = taskbar_rect()
print(f"  猛拖 -3000 ⇒ widget left = {r3[0]}, 任务栏 left = {tb4[0]}")
check("E 左缘不越出任务栏（且贴住左缘）",
      r3[0] >= tb4[0] and r3[0] - tb4[0] <= 2, f"{r3[0]} vs {tb4[0]}")

# ⭐ 最后落到一个**有辨识度的中间值**：步骤 D 要拿它证明「重启回到用户放下的地方」。
#    ⛔ 若落点是 0（= `LAST_X` 的哨兵值）或恰好等于贴靠结果，D 就失去区分力。
MID = 700
post_drag(hwnd, CX0 - 3000, CX0 - 3000 + MID)
r4 = widget_rect(hwnd)
mid_rel = r4[0] - client_origin(hwnd)[0]
print(f"  再拖到中间 ⇒ 父窗客户区 x = {mid_rel}（期望 {MID}）")
check("E 中间落点精确（±3px）", abs(mid_rel - MID) <= 3, f"{mid_rel} vs {MID}")

time.sleep(1.5)
persisted = config_value("taskbar_custom_x")
print(f"  最终 taskbar_custom_x = {persisted}（实测父窗客户区 x = {mid_rel}）")
check("E 最后一次落位也落盘了", persisted == str(mid_rel),
      f"配置 {persisted} vs 实测 {mid_rel}")

# ⭐ 日志必须在**本实例还活着**时读：`log_retention = "once"` 会让每个实例新建
#    `debug_once_<pid>.log` 并**删掉其它日志** ⇒ 脚本最后再读，前面实例的日志早没了
#    （上一版就是这样误判成「拖拽日志一条都没有」）。
print("\n=== [widget] 拖拽日志（本实例）===")
drag_lines = [ln.strip() for ln in log_text().splitlines()
              if "[widget]" in ln and "拖拽" in ln]
for ln in drag_lines:
    print("  LOG:", ln)
check("G 拖拽开始/落位都有日志（可追溯）", len(drag_lines) >= 4,
      f"{len(drag_lines)} 条（3 次拖拽 ⇒ 至少 3 开始 + 3 落位）")

# ── F ──
print("\n=== F. 固定位置（locked=true）时拖拽必须无效 ===")
proc.terminate(); time.sleep(1); kill(); time.sleep(1)
proc, hwnd = setup(locked=True, label="固定位置", clear_custom=False)
if hwnd:
    rl0 = widget_rect(hwnd)
    post_drag(hwnd, 1000, 880)
    rl1 = widget_rect(hwnd)
    print(f"  locked 下拖 -120 ⇒ left {rl0[0]} → {rl1[0]}")
    check("F locked 时窗口不动", rl1[0] == rl0[0], f"位移 {rl1[0] - rl0[0]}")
    # 顺带确认「固定位置」下仍是贴靠结果，而不是落点
    print(f"  （locked 下 rel_x = {rl0[0] - client_origin(hwnd)[0]}，"
          f"落点 {persisted} 应被忽略）")
    check("F locked 时忽略拖拽落点",
          rl0[0] - client_origin(hwnd)[0] != int(persisted or 0),
          "贴靠结果与落点不同 ⇒ 确实用的是贴靠")
else:
    check("F locked 时窗口不动", False, "找不到 widget")
    check("F locked 时忽略拖拽落点", False, "找不到 widget")

# ── D ──
print("\n=== D. 重启后回到拖拽落点 ===")
set_config_line("taskbar_position_locked", "taskbar_position_locked = false")
print(f"  启动前 config: locked = {config_value('taskbar_position_locked')}"
      f"，custom_x = {config_value('taskbar_custom_x')}")
proc.terminate(); time.sleep(1); kill(); time.sleep(1)
proc, hwnd = setup(locked=False, label="重启后解锁", clear_custom=False)
if hwnd:
    rd = widget_rect(hwnd)
    got_rel = rd[0] - client_origin(hwnd)[0]
    print(f"  重启后 父窗客户区 x = {got_rel}（配置 {persisted}）")
    check("D 重启后回到拖拽落点",
          persisted is not None and got_rel == int(persisted),
          f"实测 {got_rel} vs 配置 {persisted}")
else:
    check("D 重启后回到拖拽落点", False, "找不到 widget")

print("\n=== 收尾 ===")
proc.terminate(); time.sleep(1); kill()
shutil.copy2(CFG_BAK, CFG)
print("config 已还原")
restore()

bad = [n for n, ok, _ in RESULTS if not ok]
print(f"\n{'=' * 60}")
print(f"结果：{len(RESULTS) - len(bad)}/{len(RESULTS)} 通过")
if bad:
    print("失败项：" + "、".join(bad))
    sys.exit(1)
print("✅ 全部通过")

"""任务栏 widget 里程碑 3 验收：**配置驱动**的挂载 / 拆除 / 重定位。

本脚本要证明的是「设置页 → 窗口」这条链路**真的通了**（用户报的缺陷：
「任务栏设置页无法实际对任务栏窗口进行设置」）。四段：

  A. 默认关闭 —— `pinned_taskbar_devices` 为空 ⇒ `want=false`、窗口不存在
  B. 实时开启 —— 经**真实 IPC**（CDP 打到真实设置页）勾一台设备
                 ⇒ `config-changed` ⇒ 窗口**不重启就出现**
  C. 位置切换 —— 走**真实下拉交互**（先取引用再开浮层，见下）选「靠左」
                 ⇒ 窗口 x 变小且配置落盘
  D. 实时关闭 —— 取消勾选 ⇒ 窗口消失

⛔ 四条硬约束（踩过的坑）：
  ① 观测进程必须 **DPI 感知**（PLAYBOOK §E.3）；
  ② 启动 → 交互 → 读日志 必须在**同一个脚本**里（子进程随命令结束被回收）；
  ③ CDP 求值涉及 IPC 时**窗口必须可见**（popup 隐藏时 `invoke` 挂死）——
     故用 `PM_DEV_OPEN_SETTINGS=1` 打开设置窗，而不是隐藏的 popup；
  ④ ⚠️ **设置窗可能被真实用户随手关掉**（实测：hide 后 3s 销毁）⇒ 四段交互
     必须**紧接启动完成、尽早连做**，且每步前显式检查 target 是否还在。
"""
import subprocess, os, time, ctypes, ctypes.wintypes as w, glob, re, shutil, sys

WD = r'D:\Code\PeriTray\src-tauri\target\debug'
EXE = os.path.join(WD, 'PeriTray.exe')
CFG = os.path.join(WD, 'config.toml')
CFG_BAK = os.path.join(WD, 'config.toml.m3bak')
NODE = r'C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe'
CDP = r'D:\Code\PeriTray\tools\cdp-eval.mjs'
PORT = '9222'

u = ctypes.windll.user32

try:
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        u.SetProcessDPIAware()

FAILURES = []


def check(name, ok, detail=''):
    print(f'  [{"PASS" if ok else "FAIL"}] {name}' + (f' — {detail}' if detail else ''))
    if not ok:
        FAILURES.append(name)
    return ok


def log_text():
    logs = sorted(glob.glob(os.path.join(WD, 'logs', '*.log')), key=os.path.getmtime)
    if not logs:
        return ''
    return open(logs[-1], 'r', encoding='utf-8', errors='replace').read()


def targets():
    r = subprocess.run([NODE, CDP, '--list'], capture_output=True, text=True, timeout=30)
    return r.stdout


def settings_alive():
    return 'settings.html' in targets()


def cdp(expr):
    """在真实设置页里求值（IPC 与 DOM 都走同一条路）。"""
    r = subprocess.run([NODE, CDP, 'settings.html', expr],
                       capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise RuntimeError(f'CDP 失败: {(r.stderr or r.stdout).strip()}')
    return r.stdout.strip()


def widget_hwnd():
    """从日志里取最近一次 mount report 的 hwnd（0 表示没有存活窗口）。"""
    m = re.findall(r'mount report: hwnd=(0x[0-9a-f]+)', log_text())
    if not m:
        return 0
    h = int(m[-1], 16)
    # ⛔ 必须复核 `IsWindow`：句柄值是**历史记录**，窗口可能已被销毁
    return h if u.IsWindow(h) else 0


def widget_rect():
    h = widget_hwnd()
    if not h:
        return None
    r = w.RECT()
    u.GetWindowRect(h, ctypes.byref(r))
    return (r.left, r.top, r.right, r.bottom)


def probe_line():
    m = re.findall(r'\[widget\] 3s 后复核.*', log_text())
    return m[-1].strip() if m else ''


# ── 准备：备份配置，清空已选设备 ──────────────────────────────
subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
time.sleep(1)
shutil.copy2(CFG, CFG_BAK)
cfg = open(CFG, encoding='utf-8').read()
cfg = re.sub(r'pinned_taskbar_devices\s*=\s*\[[^\]]*\]\n?', '', cfg)
cfg = re.sub(r'taskbar_position\s*=\s*"\w+"\n?', '', cfg)
open(CFG, 'w', encoding='utf-8').write(cfg)
print('config 已重置（无已选设备、位置回默认）')

env = dict(os.environ)
env['PM_DEV_TASKBAR_WIDGET_PROBE'] = '1'   # 只开复核门控，**不**强制挂载
env['PM_DEV_OPEN_SETTINGS'] = '1'
env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = (
    '--disable-gpu-sandbox --remote-debugging-port=' + PORT)
p = subprocess.Popen([EXE], cwd=WD, env=env)
print('launched pid', p.pid)
time.sleep(9)

print('\n=== A. 默认关闭（无已选设备 ⇒ 窗口不应存在）===')
print('  probe:', probe_line()[:200])
check('A1 want=false', 'want=false' in probe_line())
check('A2 未挂载', 'mounted(ok)=false' in probe_line())
check('A3 无存活窗口', widget_hwnd() == 0)
check('A4 设置页可连（后续交互的前提）', settings_alive())

print('\n=== B. 实时开启（真实 IPC 勾选设备 ⇒ 不重启就出现）===')
try:
    picked = cdp(
        '(async () => {'
        '  const ds = await window.__TAURI__.core.invoke("get_selectable_devices");'
        '  const d = (ds || []).find(x => !x.pinned);'
        '  if (!d) return "NO-DEVICE";'
        '  await window.__TAURI__.core.invoke("toggle_pinned_taskbar_device",'
        '    { key: d.key, fallback: d.fallback ?? null, alias: null });'
        '  return d.name;'
        '})()')
except Exception as e:
    picked = f'CDP-ERROR {e}'
print('  勾选设备:', picked)
time.sleep(4)
t = log_text()
check('B1 有「按配置挂载」日志', '按配置挂载: ok=true' in t)
rect_center = widget_rect()
check('B2 窗口存在', rect_center is not None, str(rect_center))
check('B3 配置已落盘',
      'pinned_taskbar_devices' in open(CFG, encoding='utf-8').read())
# ⭐ 口径（用户 2026-09-24 拍板）：只画**已勾选**的设备。
#    可证伪：若内容源退回 `group_taskbar_devices` 的「有数据的设备一律留」，
#    本机有 9 台有数据的设备 ⇒ 窗口会宽到 ~400px（6 台）而不是「1 台」的 ~90px。
if rect_center:
    w_center = rect_center[2] - rect_center[0]
    check('B4 只画已勾选的那 1 台（宽度 ~90px，而非 6 台的 ~400px）',
          w_center < 160, f'实际宽度 {w_center}px')
    m_items = re.findall(r'\[widget\]   · \[(鼠标|音箱|耳机)\]', t)
    # ⭐ `[widget] 定位:` 里的 `content_w` 是**内容区**宽度（不含两端 PAD_X），
    #   比窗口宽度更直接地反映「画了几台」—— 6 台约 389，1 台约 77。
    mc = re.findall(r'\[widget\] 定位:.*content_w=(\d+)', t)
    check('B5 定位日志的 content_w 是「1 台」的量级（< 150）',
          bool(mc) and int(mc[-1]) < 150, f'content_w={mc[-1] if mc else "?"}')
    print('  （诊断条目数样本，仅参考:', len(m_items), '条）')

print('\n=== C. 位置切换（走真实下拉交互，center → left）===')
print('  设置页还在:', settings_alive())
# ⛔ 必须先取 item 引用再点开按钮：`initComboBox` 打开时会把 flyout **移到 body**，
#    之后 `combo.querySelector(...)` 就找不到它了（本仓已知坑）。
try:
    label = cdp(
        '(() => {'
        '  const combo = document.getElementById("combo-taskbar-position");'
        '  const item = combo.querySelector(\'.win-combo-item[data-value="left"]\');'
        '  combo.querySelector(".win-combo-btn").click();'
        '  item.click();'
        '  return document.querySelector("#combo-taskbar-position .win-combo-content").textContent;'
        '})()')
except Exception as e:
    label = f'CDP-ERROR {e}'
print('  下拉文案:', label)
time.sleep(4)
rect_left = widget_rect()
m = re.search(r'taskbar_position\s*=\s*"(\w+)"', open(CFG, encoding='utf-8').read())
check('C1 下拉文案已变为「靠左」', label == '靠左', label)
check('C2 配置已落盘 taskbar_position=left', bool(m) and m.group(1) == 'left',
      m.group(1) if m else '未找到')
check('C3 窗口存在', rect_left is not None, str(rect_left))
if rect_center and rect_left:
    check('C4 靠左后 x 变小（真的重定位了）', rect_left[0] < rect_center[0],
          f'{rect_center[0]} → {rect_left[0]}')

print('\n=== D. 实时关闭（取消勾选 ⇒ 窗口消失）===')
print('  设置页还在:', settings_alive())
try:
    out = cdp(
        '(async () => {'
        '  const ds = await window.__TAURI__.core.invoke("get_selectable_devices");'
        '  const d = (ds || []).find(x => x.pinned);'
        '  if (!d) return "NONE-PINNED";'
        '  await window.__TAURI__.core.invoke("toggle_pinned_taskbar_device",'
        '    { key: d.key, fallback: d.fallback ?? null, alias: null });'
        '  return d.name;'
        '})()')
except Exception as e:
    out = f'CDP-ERROR {e}'
print('  取消勾选:', out)
time.sleep(3)
t = log_text()
check('D1 有「按配置拆除」日志', '按配置拆除' in t)
check('D2 窗口已消失', widget_hwnd() == 0)

print('\n--- 相关日志 ---')
for line in re.findall(r'\[widget\].*', log_text())[-14:]:
    print('  ', line.strip()[:170])

p.terminate()
time.sleep(1)
subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
# ⛔ 用 `copy2` 覆盖而不是 `move`：`shutil.move` 在 Windows 上会走
#    「copy + unlink」兜底，而本机 `unlink` 被 safe-delete 守卫拦死。
shutil.copy2(CFG_BAK, CFG)
print('\nconfig 已还原')

print('\n=== 汇总 ===')
if FAILURES:
    print('FAILED:', FAILURES)
    sys.exit(1)
print('全部通过')

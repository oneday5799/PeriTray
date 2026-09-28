"""任务栏 widget 里程碑 2 视觉验证（真实内容：设备名 + 电量 + 音量）。

与 verify-widget.py（里程碑 1）的区别：本脚本**不只判断「有没有黑块」**，
而是验证「画的是**文本**」—— 判据是：
  · 内容区有**大量不同灰度**的像素（文本抗锯齿的边缘 ⇒ 灰度分布，而非纯色块）；
  · 内容区**宽度 > 里程碑 1 的 120px**（真实文本比 8x8 方块宽）；
  · 冒烟：`[widget] mount report` 的 `drawn=true` 且日志里有「快照更新」。

⛔ 三个必须遵守的观测条件（踩过的坑，见 PLAYBOOK §E.3）：
  ① BitBlt 抓**分层窗口**必须 `CAPTUREBLT`；
  ② 观测进程必须 **DPI 感知**（否则截图整体错位）；
  ③ 启动 → 读日志 → 截图 → 分析 必须在**同一个脚本**里（子进程随命令结束被回收）。
"""
import subprocess, os, time, ctypes, ctypes.wintypes as w, glob, re, zlib, struct
from collections import Counter

WD = r'D:\Code\PeriTray\src-tauri\target\debug'
EXE = os.path.join(WD, 'PeriTray.exe')
SCRATCH = r'D:\Code\PeriTray\tools\local\_out'

CAPTUREBLT = 0x40000000
SRCCOPY = 0x00CC0020

u = ctypes.windll.user32
g = ctypes.windll.gdi32

# ⛔ DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4 （见 verify-widget.py 注释）
try:
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        u.SetProcessDPIAware()


def launch(wait=12):
    env = dict(os.environ)
    env['PM_DEV_TASKBAR_WIDGET'] = '1'
    env['PM_DEV_TASKBAR_WIDGET_PROBE'] = '1'
    env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--disable-gpu-sandbox'
    subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
    time.sleep(1)
    p = subprocess.Popen([EXE], cwd=WD, env=env)
    # ⚠️ 首次刷新有 3s 延迟 + WMI 600ms ⇒ 给足时间确保快照已落盘
    time.sleep(wait)
    return p


def read_log():
    logs = sorted(glob.glob(os.path.join(WD, 'logs', 'debug_once_*.log')), key=os.path.getmtime)
    return logs[-1], open(logs[-1], 'r', encoding='utf-8', errors='replace').read()


def grab(y0, height):
    cx = u.GetSystemMetrics(0)
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    hbm = g.CreateCompatibleBitmap(hdc, cx, height)
    g.SelectObject(mem, hbm)
    g.BitBlt(mem, 0, 0, cx, height, hdc, 0, y0, SRCCOPY | CAPTUREBLT)

    class BIH(ctypes.Structure):
        _fields_ = [('biSize', w.DWORD), ('biWidth', w.LONG), ('biHeight', w.LONG),
                    ('biPlanes', w.WORD), ('biBitCount', w.WORD), ('biCompression', w.DWORD),
                    ('biSizeImage', w.DWORD), ('biXPelsPerMeter', w.LONG),
                    ('biYPelsPerMeter', w.LONG), ('biClrUsed', w.DWORD), ('biClrImportant', w.DWORD)]
    bi = BIH()
    bi.biSize = ctypes.sizeof(BIH)
    bi.biWidth = cx
    bi.biHeight = -height
    bi.biPlanes = 1
    bi.biBitCount = 32
    buf = ctypes.create_string_buffer(cx * height * 4)
    g.GetDIBits(mem, hbm, 0, height, buf, ctypes.byref(bi), 0)
    g.DeleteObject(hbm)
    g.DeleteDC(mem)
    u.ReleaseDC(0, hdc)
    raw = buf.raw

    def px(x, y):
        i = (y * cx + x) * 4
        return (raw[i + 2], raw[i + 1], raw[i])
    return cx, px


def save_png(path, w_, h_, getpx):
    raw = b''
    for y in range(h_):
        raw += b'\x00' + b''.join(bytes(getpx(x, y)) for x in range(w_))

    def chunk(t, d):
        c = t + d
        return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    png = b'\x89PNG\r\n\x1a\n'
    png += chunk(b'IHDR', struct.pack('>IIBBBBB', w_, h_, 8, 2, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(raw, 6))
    png += chunk(b'IEND', b'')
    open(path, 'wb').write(png)


p = launch()
print('launched pid', p.pid, 'alive=', p.poll() is None)

logf, log = read_log()
print('log:', logf)

# ── 日志判据 ────────────────────────────────────────────
mounts = re.findall(
    r'mount report: hwnd=(0x[0-9a-f]+) taskbar=(0x[0-9a-f]+) reparent_err=(\d+) '
    r'getparent=(0x[0-9a-f]+) ok=(\w+) drawn=(\w+)', log)
snaps = re.findall(r'快照更新 \((\d+) 台, (\d+)ms\)', log)
listeners = re.findall(r'已订阅 (\d+) 个数据变更事件', log)
probes = re.findall(r'PROBE.*', log)

print('--- mount ---')
print(mounts[-1] if mounts else 'NO MOUNT LINE')
print('--- snapshot updates ---', snaps)
print('--- listeners ---', listeners)
for line in probes[-3:]:
    print('--- probe ---', line.strip()[:220])

if not mounts:
    print('FATAL: 没有 mount 行'); p.terminate(); raise SystemExit(1)

hwnd = int(mounts[-1][0], 16)
print('IsWindow:', bool(u.IsWindow(hwnd)))
r = w.RECT()
u.GetWindowRect(hwnd, ctypes.byref(r))
ww, wh = r.right - r.left, r.bottom - r.top
print(f'widget rect: {r.left},{r.top}..{r.right},{r.bottom}  size {ww} x {wh}')
print('GetParent:', hex(u.GetParent(hwnd)))

tray = w.RECT()
u.GetWindowRect(u.FindWindowW('Shell_TrayWnd', None), ctypes.byref(tray))
print('tray:', tray.left, tray.top, tray.right, tray.bottom)
print('widget.top == tray.top:', r.top == tray.top)

# ── 截图分析 ────────────────────────────────────────────
y0 = tray.top
H = tray.bottom - tray.top
cx, px = grab(y0, H)
wy1, wy2 = r.top - y0, r.bottom - y0

# 在 widget 区域内统计
cnt = Counter()
grays = Counter()
nonbg = 0
for yy in range(max(0, wy1), min(H, wy2)):
    for xx in range(max(0, r.left), min(cx, r.right)):
        c = px(xx, yy)
        cnt[c] += 1
        # 灰度（文本黑/白 + 抗锯齿中间灰 ⇒ 会散开成很多灰度值）
        lum = (c[0] * 299 + c[1] * 587 + c[2] * 114) // 1000
        grays[lum] += 1

# 「内容像素」= 与任务栏底色明显不同的像素（底色本机实测约 (28,28,28) 深色任务栏）
# ⚠️ 深色任务栏下文本是白色 ⇒ 用「亮度 > 100」判内容；浅色主题反过来。
bg = cnt.most_common(1)[0][0]
content = sum(v for k, v in cnt.items()
              if abs(k[0] - bg[0]) + abs(k[1] - bg[1]) + abs(k[2] - bg[2]) > 30)
distinct_grays = len([v for v in grays.values() if v > 0])

print(f'widget size: {ww}x{wh}')
print('bg color (mode):', bg)
print('distinct colors in widget:', len(cnt), ' distinct gray levels:', distinct_grays)
print('content pixels (vs bg):', content)
print('top colors:', cnt.most_common(6))

save_png(os.path.join(SCRATCH, 'taskbar-widget-m2.png'), cx, H, px)
# 另存一张只含 widget 区域的放大图（便于肉眼核对文本）
crop_w, crop_h = min(cx, r.right) - max(0, r.left), min(H, wy2) - max(0, wy1)
if crop_w > 0 and crop_h > 0:
    def crop_px(x, y):
        return px(max(0, r.left) + x, max(0, wy1) + y)
    save_png(os.path.join(SCRATCH, 'widget-crop-m2.png'), crop_w, crop_h, crop_px)
print('saved taskbar-widget-m2.png + widget-crop-m2.png')

print('--- verdict ---')
print('mounted_ok    :', mounts[-1][4] == 'true')
print('drawn_ok      :', mounts[-1][5] == 'true')
print('snapshot_seen :', len(snaps) > 0, snaps)
print('listeners_ok  :', listeners == ['6'], listeners)
print('text_likely   :', distinct_grays > 8 and content > 200)

p.terminate()
time.sleep(0.5)
print('done')

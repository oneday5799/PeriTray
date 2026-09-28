"""任务栏 widget 视觉验证（启动 → 存活期截图 → 像素分析）。

用途：验证 B3-A 里程碑 1 —— widget 是否真的显示在任务栏上、位置/尺寸/透明背景是否正确。

关键点（踩过的坑）：
  ① BitBlt 抓**分层窗口**必须加 `CAPTUREBLT` (0x40000000)，否则 ULW 绘制的窗口**抓不到**；
  ② 采样必须覆盖**内容块所在行**（DIB 内 y=4..12），别只采窗口中部；
  ③ 窗口必须与**宿主进程同生共死** ⇒ 启动、截图、分析必须在**同一进程内**完成，
     否则进程被回收后 hwnd 全部失效（`IsWindow=False`）。
"""
import subprocess, os, time, ctypes, ctypes.wintypes as w, glob, re, zlib, struct

WD = r'D:\Code\PeriTray\src-tauri\target\debug'
EXE = os.path.join(WD, 'PeriTray.exe')
SCRATCH = r'D:\Code\PeriTray\tools\local\_out'

CAPTUREBLT = 0x40000000
SRCCOPY = 0x00CC0020

u = ctypes.windll.user32
g = ctypes.windll.gdi32

# ⛔⛔ 必须把本进程设为 **DPI 感知**，否则 `GetSystemMetrics` / `GetWindowRect` 返回的是
#    被缩放的**逻辑坐标**，而 `BitBlt` 用的是**物理坐标** ⇒ 截图位置整体错位
#    （实测：125% 缩放下任务栏逻辑 top=1104，物理 top=1380，截出来的是编辑器内容）。
#    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 = -4
try:
    ctypes.windll.user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    try:
        ctypes.windll.shcore.SetProcessDpiAwareness(2)
    except Exception:
        u.SetProcessDPIAware()


def launch():
    env = dict(os.environ)
    env['PM_DEV_TASKBAR_WIDGET'] = '1'
    env['PM_DEV_TASKBAR_WIDGET_PROBE'] = '1'
    env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--disable-gpu-sandbox --remote-debugging-port=9222'
    subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
    time.sleep(1)
    p = subprocess.Popen([EXE], cwd=WD, env=env)
    time.sleep(9)
    return p


def read_mount():
    logs = sorted(glob.glob(os.path.join(WD, 'logs', 'debug_once_*.log')), key=os.path.getmtime)
    log = open(logs[-1], 'r', encoding='utf-8', errors='replace').read()
    m = re.findall(r'mount report: hwnd=(0x[0-9a-f]+) taskbar=(0x[0-9a-f]+) reparent_err=(\d+) getparent=(0x[0-9a-f]+) ok=(\w+) drawn=(\w+)', log)
    probe = re.findall(r'(\d+s 后复核): ok=(\w+)', log)
    return logs[-1], m, probe


def grab_strip(y0, height):
    cx, cy = u.GetSystemMetrics(0), u.GetSystemMetrics(1)
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    hbm = g.CreateCompatibleBitmap(hdc, cx, height)
    g.SelectObject(mem, hbm)
    # ⛔ CAPTUREBLT: 否则抓不到分层窗口（ULW 绘制的内容）
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

    def px(x, y):
        i = (y * cx + x) * 4
        return (buf.raw[i + 2], buf.raw[i + 1], buf.raw[i], buf.raw[i + 3])
    return cx, height, px


def save_png(path, w_, h_, getpx):
    raw = b''
    for y in range(h_):
        raw += b'\x00' + b''.join(bytes(getpx(x, y)[:3]) for x in range(w_))

    def chunk(t, d):
        c = t + d
        return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    png = b'\x89PNG\r\n\x1a\n'
    png += chunk(b'IHDR', struct.pack('>IIBBBBB', w_, h_, 8, 2, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(raw, 6))
    png += chunk(b'IEND', b'')
    open(path, 'wb').write(png)


p = launch()
print('launched pid', p.pid, 'poll=', p.poll())
logf, mounts, probes = read_mount()
print('log:', logf)
print('mount:', mounts[-1] if mounts else None)
print('probe:', probes)

hwnd = int(mounts[-1][0], 16)
alive = bool(u.IsWindow(hwnd))
print('IsWindow:', alive)
r = w.RECT()
u.GetWindowRect(hwnd, ctypes.byref(r))
print('widget rect:', r.left, r.top, r.right, r.bottom, 'size', r.right - r.left, 'x', r.bottom - r.top)
print('GetParent:', hex(u.GetParent(hwnd)))

tray = w.RECT()
u.GetWindowRect(u.FindWindowW('Shell_TrayWnd', None), ctypes.byref(tray))
print('tray rect:', tray.left, tray.top, tray.right, tray.bottom)

# 抓整个任务栏高度
y0 = tray.top
H = tray.bottom - tray.top
cx, hh, px = grab_strip(y0, H)

# 分析 widget 区域
wx1, wx2 = r.left, r.right
wy1, wy2 = r.top - y0, r.bottom - y0
print(f'widget within strip: x {wx1}..{wx2}, y {wy1}..{wy2}')

from collections import Counter
cnt = Counter()
black = 0
for yy in range(max(0, wy1), min(hh, wy2)):
    for xx in range(max(0, wx1), min(cx, wx2)):
        rr, gg, bb, aa = px(xx, yy)
        cnt[(rr, gg, bb)] += 1
        if rr < 40 and gg < 40 and bb < 40:
            black += 1
print('widget area top colors:', cnt.most_common(5))
print('near-black pixels:', black)

# 也抓整个屏幕任务栏条保存
save_png(os.path.join(SCRATCH, 'taskbar-strip.png'), cx, hh, px)
print('saved taskbar-strip.png', cx, 'x', hh)

p.terminate()
time.sleep(0.5)
print('done')

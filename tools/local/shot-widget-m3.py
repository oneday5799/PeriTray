"""任务栏 widget 最终效果截图：同一份内容在 center / left 两种贴靠下的样子。

产出：`widget-m3-center.png`（任务栏整条，含 widget）、`widget-m3-left.png`、
以及放大裁剪 `widget-m3-crop.png`（便于肉眼核对图标与数值）。

⚠️ 只截「任务栏那一条」而不是整屏 —— 减少对用户的打扰，也避免泄露无关窗口内容。
"""
import subprocess, os, time, ctypes, ctypes.wintypes as w, glob, re, shutil, struct, zlib

WD = r'D:\Code\PeriTray\src-tauri\target\debug'
EXE = os.path.join(WD, 'PeriTray.exe')
CFG = os.path.join(WD, 'config.toml')
CFG_BAK = os.path.join(WD, 'config.toml.m3bak')
NODE = r'C:\Users\Oneday\.workbuddy-ai\binaries\node\versions\22.22.2-3\node.exe'
CDP = r'D:\Code\PeriTray\tools\cdp-eval.mjs'
OUT = r'D:\Code\PeriTray\tools\local\_out'

CAPTUREBLT, SRCCOPY = 0x40000000, 0x00CC0020
u, g = ctypes.windll.user32, ctypes.windll.gdi32
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    u.SetProcessDPIAware()


class BIH(ctypes.Structure):
    _fields_ = [('biSize', w.DWORD), ('biWidth', w.LONG), ('biHeight', w.LONG),
                ('biPlanes', w.WORD), ('biBitCount', w.WORD), ('biCompression', w.DWORD),
                ('biSizeImage', w.DWORD), ('biXPelsPerMeter', w.LONG),
                ('biYPelsPerMeter', w.LONG), ('biClrUsed', w.DWORD), ('biClrImportant', w.DWORD)]


def grab(y0, height):
    cx = u.GetSystemMetrics(0)
    hdc = u.GetDC(0)
    mem = g.CreateCompatibleDC(hdc)
    hbm = g.CreateCompatibleBitmap(hdc, cx, height)
    g.SelectObject(mem, hbm)
    g.BitBlt(mem, 0, 0, cx, height, hdc, 0, y0, SRCCOPY | CAPTUREBLT)
    bi = BIH()
    bi.biSize, bi.biWidth, bi.biHeight = ctypes.sizeof(BIH), cx, -height
    bi.biPlanes, bi.biBitCount = 1, 32
    buf = ctypes.create_string_buffer(cx * height * 4)
    g.GetDIBits(mem, hbm, 0, height, buf, ctypes.byref(bi), 0)
    g.DeleteObject(hbm)
    g.DeleteDC(mem)
    u.ReleaseDC(0, hdc)
    raw = buf.raw
    return cx, lambda x, y: (raw[(y * cx + x) * 4 + 2], raw[(y * cx + x) * 4 + 1], raw[(y * cx + x) * 4])


def save_png(path, w_, h_, getpx, scale=1):
    raw = b''
    for y in range(h_):
        row = b''.join(bytes(getpx(x, y)) * scale for x in range(w_))
        raw += (b'\x00' + row) * scale

    def chunk(t, d):
        c = t + d
        return struct.pack('>I', len(d)) + c + struct.pack('>I', zlib.crc32(c) & 0xffffffff)
    png = b'\x89PNG\r\n\x1a\n'
    png += chunk(b'IHDR', struct.pack('>IIBBBBB', w_ * scale, h_ * scale, 8, 2, 0, 0, 0))
    png += chunk(b'IDAT', zlib.compress(raw, 6))
    png += chunk(b'IEND', b'')
    open(path, 'wb').write(png)


def log_text():
    logs = sorted(glob.glob(os.path.join(WD, 'logs', '*.log')), key=os.path.getmtime)
    return open(logs[-1], 'r', encoding='utf-8', errors='replace').read() if logs else ''


def widget_hwnd():
    m = re.findall(r'mount report: hwnd=(0x[0-9a-f]+)', log_text())
    if not m:
        return 0
    h = int(m[-1], 16)
    return h if u.IsWindow(h) else 0


def cdp(expr):
    r = subprocess.run([NODE, CDP, 'settings.html', expr], capture_output=True, text=True, timeout=60)
    if r.returncode != 0:
        raise RuntimeError((r.stderr or r.stdout).strip())
    return r.stdout.strip()


subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
time.sleep(1)
shutil.copy2(CFG, CFG_BAK)
cfg = re.sub(r'pinned_taskbar_devices\s*=\s*\[[^\]]*\]\n?', '',
             open(CFG, encoding='utf-8').read())
cfg = re.sub(r'taskbar_position\s*=\s*"\w+"\n?', '', cfg)
open(CFG, 'w', encoding='utf-8').write(cfg)

env = dict(os.environ)
env['PM_DEV_OPEN_SETTINGS'] = '1'
env['WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS'] = '--disable-gpu-sandbox --remote-debugging-port=9222'
p = subprocess.Popen([EXE], cwd=WD, env=env)
time.sleep(9)

PIN = ('(async () => {'
       '  const ds = await window.__TAURI__.core.invoke("get_selectable_devices");'
       '  const d = (ds || []).find(x => !x.pinned);'
       '  await window.__TAURI__.core.invoke("toggle_pinned_taskbar_device",'
       '    { key: d.key, fallback: d.fallback ?? null, alias: null });'
       '  return d.name; })()')
print('pin:', cdp(PIN))
time.sleep(5)

tray = w.RECT()
u.GetWindowRect(u.FindWindowW('Shell_TrayWnd', None), ctypes.byref(tray))
H = tray.bottom - tray.top


def shot(tag):
    cx, px = grab(tray.top, H)
    save_png(os.path.join(OUT, f'widget-m3-{tag}.png'), cx, H, px)
    # ⛔ 不能用 `FindWindowW(类名)`：widget 是**子窗**，`FindWindowW` 只找顶层窗。
    #    句柄一律从日志的 mount report 取（与验收脚本同一口径）。
    hwnd = widget_hwnd()
    if hwnd:
        r = w.RECT()
        u.GetWindowRect(hwnd, ctypes.byref(r))
        cw = min(cx, r.right) - max(0, r.left)
        ch = min(H, r.bottom - tray.top) - max(0, r.top - tray.top)
        if cw > 0 and ch > 0:
            save_png(os.path.join(OUT, f'widget-m3-crop-{tag}.png'), cw, ch,
                     lambda x, y: px(max(0, r.left) + x, max(0, r.top - tray.top) + y), scale=4)
        print(f'{tag}: widget rect {r.left},{r.top}..{r.right},{r.bottom}  size {r.right-r.left}x{r.bottom-r.top}')
    else:
        print(f'{tag}: 找不到 widget 窗口')


shot('center')
cdp('(() => { const c = document.getElementById("combo-taskbar-position");'
    ' const it = c.querySelector(\'.win-combo-item[data-value="left"]\');'
    ' c.querySelector(".win-combo-btn").click(); it.click(); return 1; })()')
time.sleep(5)
shot('left')

p.terminate()
time.sleep(1)
subprocess.run(['taskkill', '/F', '/IM', 'PeriTray.exe'], capture_output=True)
shutil.copy2(CFG_BAK, CFG)
print('config 已还原')

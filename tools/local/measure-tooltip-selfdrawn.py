"""量自绘 tooltip 的实际像素，对齐 FluentFlyout `CustomToolTip.xaml`。

判据（都是**会随实现变化**的量，不是「修复前后相同」的回归项）：
· 气泡尺寸      ≈ 106×24
· 圆角半径      = 底边起点的左移量（`CornerRadius=4` DIP × 本机内容缩放）
· 边框 / 底色   = #E5E5E5 / #F9F9F9
· 文本内边距    左 8 / 上 5（1px 边框 + Padding）
· 阴影          气泡**上方**必须有羽化像素（`DropShadowEffect Direction=270`）
· 与任务栏间距  = 6~8px

⛔ 两个**量脚本自身**的坑（都曾导致假结论，别再踩）：
1. 必须在**窗口矩形内**取样：桌面壁纸里存在与气泡底色**完全相同**的 #F9F9F9。
2. `gray()` 已经返回 0..255 的**均值**，阈值必须按 0..255 写。
   初版写成 `sum3 < 560` ⇒ 恒成立 ⇒ 「文本」= 整个气泡（2288 px），
   差点报成「文字铺满气泡」。
"""
import struct
import sys
import zlib

sys.stdout.reconfigure(encoding="utf-8")

OX, OY = 1167, 1324            # 截图裁剪原点
WIN_W, WIN_H = 138, 56         # 窗口尺寸（app 日志）
BUBBLE_W, BUBBLE_H = 106, 24   # 气泡尺寸（app 日志）
PAD = 16                       # TIP_SHADOW_PAD_PX
BG = (249, 249, 249)
BORDER = (229, 229, 229)
TASKBAR_TOP = 1380


def load(path):
    d = open(path, "rb").read()
    i, w, h, idat = 8, 0, 0, b""
    while i < len(d):
        ln = struct.unpack(">I", d[i:i + 4])[0]
        tag = d[i + 4:i + 8]
        if tag == b"IHDR":
            w, h = struct.unpack(">II", d[i + 8:i + 16])
        if tag == b"IDAT":
            idat += d[i + 8:i + 8 + ln]
        i += 12 + ln
    raw = zlib.decompress(idat)
    stride, prev, pos, px = w * 3, bytearray(w * 3), 0, []
    for _ in range(h):
        f = raw[pos]
        pos += 1
        line = bytearray(raw[pos:pos + stride])
        pos += stride
        for x in range(stride):
            a = line[x - 3] if x >= 3 else 0
            b = prev[x]
            c = prev[x - 3] if x >= 3 else 0
            if f == 1:
                line[x] = (line[x] + a) & 255
            elif f == 2:
                line[x] = (line[x] + b) & 255
            elif f == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif f == 4:
                pp = a + b - c
                pa, pb, pc = abs(pp - a), abs(pp - b), abs(pp - c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        px.append(bytes(line))
        prev = line
    return px, w, h


def main():
    px, w, h = load(
        "D:/Code/PeriTray/tools/local/shot-tooltip-actual.png")
    at = lambda x, y: (px[y][x * 3], px[y][x * 3 + 1], px[y][x * 3 + 2])
    gray = lambda x, y: sum(at(x, y)) // 3

    wx0, wy0 = 10, 10
    wx1, wy1 = wx0 + WIN_W, wy0 + WIN_H
    bx0, by0 = wx0 + PAD, wy0 + PAD
    bx1, by1 = bx0 + BUBBLE_W, by0 + BUBBLE_H
    ok = []

    cols = [x for x in range(bx0, bx1)
            if any(at(x, y) == BG for y in range(by0, by1))]
    rows = [y for y in range(by0, by1)
            if any(at(x, y) == BG for x in range(bx0, bx1))]
    L, R, T, B = min(cols), max(cols), min(rows), max(rows)
    v = (R - L + 1, B - T + 1)
    good = abs(v[0] - BUBBLE_W) <= 2 and abs(v[1] - BUBBLE_H) <= 2
    ok.append(good)
    print("[1] 气泡        %dx%d  (log %dx%d)  %s"
          % (v[0], v[1], BUBBLE_W, BUBBLE_H, "OK" if good else "偏离"))

    # 圆角半径 = 底边 bg 起点相对「中间行 bg 起点」的左移量
    def row_left(y):
        return next(x for x in range(bx0, bx1) if at(x, y) == BG)

    mid = (T + B) // 2
    r = row_left(mid) - row_left(B)
    ok.append(2 <= r <= 6)
    print("[2] 圆角半径    %d px  (CornerRadius=4 DIP x 本机内容缩放)  %s"
          % (r, "OK" if 2 <= r <= 6 else "偏离"))

    rim = {}
    for y in range(T - 1, B + 2):
        for x in range(L - 1, R + 2):
            c = at(x, y)
            if c != BG:
                rim[c] = rim.get(c, 0) + 1
    dom = max(rim, key=rim.get)
    ok.append(dom == BORDER)
    print("[3] 边框主色    %s x%d  %s"
          % (dom, rim[dom], "OK #E5E5E5" if dom == BORDER else "偏离"))

    ink = [(x, y) for y in range(T, B + 1) for x in range(L, R + 1)
           if gray(x, y) < 187]
    ix = [a for a, _ in ink]
    iy = [b for _, b in ink]
    pl, pt = min(ix) - L, min(iy) - T
    good = 6 <= pl <= 10 and 4 <= pt <= 7
    ok.append(good)
    print("[4] 文本        %d px  宽%d 高%d  左内边距%d 上内边距%d  %s"
          % (len(ink), max(ix) - min(ix) + 1, max(iy) - min(iy) + 1,
             pl, pt, "OK" if good else "偏离"))

    sh = [(x, y) for y in range(wy0, T) for x in range(wx0, wx1)
          if 150 < gray(x, y) < 235]
    good = len(sh) > 200
    ok.append(good)
    if sh:
        ys = [b for _, b in sh]
        print("[5] 上方阴影    %d px  纵深%d  最暗%d  %s"
              % (len(sh), max(ys) - min(ys) + 1,
                 min(gray(a, b) for a, b in sh), "OK 羽化" if good else "过弱"))
    else:
        print("[5] 上方阴影    0 px  ❌ 缺失")

    gap = TASKBAR_TOP - (OY + B + 1)
    good = 5 <= gap <= 9
    ok.append(good)
    print("[6] 任务栏间距  %d px  (6 DIP x 本机内容缩放)  %s"
          % (gap, "OK" if good else "偏离"))

    print("\n%s  %d/%d 项通过"
          % ("✅ 全部通过" if all(ok) else "❌ 有偏离", sum(ok), len(ok)))


if __name__ == "__main__":
    main()

"""诊断 tooltip 文字是否真的画出来，以及最暗像素有多暗。

用法：先跑 shot-tooltip.py，再跑本脚本。
⛔ 与上一版量脚本同一个坑：`at()` 必须在**窗口矩形内**取样（壁纸里也有 #FCFCFC）。
"""
import struct
import sys
import zlib

sys.stdout.reconfigure(encoding="utf-8")

ORIGIN = (1091, 1314)   # shot-tooltip.py 打印的「抓取原点」
WIN = (1101, 1324, 1256, 1392)  # tip rect（绝对）
PAD = 18                # 浅色主题的画布边距：max(|0|,|8|,16) + 2


def load(p):
    d = open(p, "rb").read()
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
    ox, oy = ORIGIN
    wx, wy = WIN[0] - ox, WIN[1] - oy
    print("窗口 crop(%d,%d) %dx%d" % (wx, wy, WIN[2] - WIN[0], WIN[3] - WIN[1]))
    bx0, by0 = wx + PAD, wy + PAD
    bw = (WIN[2] - WIN[0]) - 2 * PAD
    bh = (WIN[3] - WIN[1]) - 2 * PAD
    print("气泡 crop(%d,%d) %dx%d" % (bx0, by0, bw, bh))

    is_bg = lambda c: c[0] > 245 and abs(c[0] - c[1]) < 3 and abs(c[1] - c[2]) < 3
    rows = [y for y in range(by0, by0 + bh)
            if sum(1 for x in range(bx0, bx0 + bw) if is_bg(at(x, y))) > bw // 2]
    if not rows:
        print("❌ 没找到气泡底色行")
        return
    T, B = min(rows), max(rows)
    print("气泡实测行 %d..%d" % (T, B))

    dark = [(x, y, at(x, y)) for y in range(T + 2, B - 1)
            for x in range(bx0 + 2, bx0 + bw - 2)
            if sum(at(x, y)) / 3 < 200]
    if not dark:
        print("❌ 气泡内**没有任何**暗像素 ⇒ 文字没画出来")
        return
    xs = [a for a, _, _ in dark]
    ys = [b for _, b, _ in dark]
    gmin = min(dark, key=lambda t: sum(t[2]))[2]
    print("文字 %d px  x %d..%d (w=%d)  y %d..%d (h=%d)  最暗 %s (灰%d)"
          % (len(dark), min(xs), max(xs), max(xs) - min(xs) + 1,
             min(ys), max(ys), max(ys) - min(ys) + 1, gmin, sum(gmin) / 3))


if __name__ == "__main__":
    main()

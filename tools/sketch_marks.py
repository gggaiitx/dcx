#!/usr/bin/env python3
"""DCX 图标草图 · 第三轮（实体字形路线）

前两轮的通病：几个细白线框拼在一起，缺少主角。这一轮统一改成"一个粗壮字形
+ 材质表面"，接近 Win11 自带图标（设置 / 计算器）的做法。

    python tools/sketch_marks.py
"""

import importlib.util
import math
from pathlib import Path

from PIL import Image, ImageDraw, ImageChops, ImageFilter

HERE = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("gb", HERE / "gen_brand.py")
gb = importlib.util.module_from_spec(spec)
spec.loader.exec_module(gb)

OUT = HERE.parent / "brand" / "sketches"
OUT.mkdir(parents=True, exist_ok=True)
PAD = 0.055


def arc_pts(cx, cy, r, a0, a1, n=200):
    return [(cx + r * math.cos(math.radians(a0 + (a1 - a0) * i / n)),
             cy + r * math.sin(math.radians(a0 + (a1 - a0) * i / n))) for i in range(n + 1)]


def shift(m, dx, dy):
    return ImageChops.offset(m, int(dx), int(dy))


def _solid(S, color, mask):
    lay = Image.new("RGBA", (S, S), color)
    lay.putalpha(ImageChops.multiply(lay.split()[3], mask))
    return lay


def capsule_poly(cx, cy, L, t, deg):
    """Rotated capsule outline (L = overall length incl. caps, t = thickness)."""
    a = math.radians(deg)
    r = t / 2
    d = L / 2 - r
    c1 = (cx - d * math.cos(a), cy - d * math.sin(a))
    c2 = (cx + d * math.cos(a), cy + d * math.sin(a))
    pts = arc_pts(c1[0], c1[1], r, deg + 90, deg + 270)
    pts += arc_pts(c2[0], c2[1], r, deg - 90, deg + 90)
    return pts


def tile(S, deep=False):
    pad = S * PAD
    m = gb.tile_mask(S, pad)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    if deep:
        img.paste(_deep_gradient(S), (0, 0), m)
    else:
        img.paste(gb.gradient(S), (0, 0), m)
    rim = ImageChops.multiply(ImageChops.subtract(m, shift(m, 0, S * 0.014)), m)
    img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 58), rim))
    low = ImageChops.multiply(ImageChops.subtract(m, shift(m, 0, -S * 0.024)), m)
    low = low.filter(ImageFilter.GaussianBlur(S * 0.007))
    img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 46), low))
    return img


def _deep_gradient(S):
    from PIL import ImageChops as C
    lh = Image.linear_gradient("L").rotate(-90, expand=True).resize((S, S))
    lv = Image.linear_gradient("L").resize((S, S))
    diag = C.add(lh, lv, scale=2.0)
    stops = []
    a, b, c = (0x2E, 0x86, 0xF0), (0x0B, 0x4C, 0xA8), (0x05, 0x21, 0x55)
    for i in range(256):
        t = i / 255.0
        stops.append(gb.lerp(a, b, t / 0.55) if t < 0.55 else gb.lerp(b, c, (t - 0.55) / 0.45))
    return Image.merge("RGB", (diag.point(lambda v: stops[v][0]),
                               diag.point(lambda v: stops[v][1]),
                               diag.point(lambda v: stops[v][2])))


def mark(img, m, alpha=255, shadow=True):
    S = img.size[0]
    if shadow:
        sh = shift(m.filter(ImageFilter.GaussianBlur(S * 0.013)), 0, S * 0.011)
        img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 55), sh))
    return Image.alpha_composite(img, _solid(S, (255, 255, 255, alpha), m))


# ---------------------------------------------------------------- P1 实心窗 + 负形 dock
def p1(S, pad):
    T = S - 2 * pad
    W, H = 0.60 * T, 0.56 * T
    x0, y0 = (S - W) / 2, (S - H) / 2
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([x0, y0, x0 + W, y0 + H], radius=0.13 * T, fill=255)
    cw, ch = 0.34 * T, 0.105 * T
    d.rounded_rectangle([x0 + (W - cw) / 2, y0 + H - 0.135 * T - ch,
                         x0 + (W + cw) / 2, y0 + H - 0.135 * T], radius=ch / 2, fill=0)
    return mark(tile(S), m)


# ---------------------------------------------------------------- P2 D 字标（加重）
def p2(S, pad):
    T = S - 2 * pad
    H, W = 0.62 * T, 0.47 * T
    w = 0.235 * H
    x0, y0 = (S - W) / 2, (S - H) / 2
    cx, cy = x0 + W - H / 2, y0 + H / 2
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.polygon([(x0, y0), (cx, y0)] + arc_pts(cx, cy, H / 2, -90, 90) + [(x0, y0 + H)], fill=255)
    r_in = H / 2 - w
    c = Image.new("L", (S, S), 0)
    dc = ImageDraw.Draw(c)
    dc.polygon([(x0 + w, y0 + w), (cx, y0 + w)] + arc_pts(cx, cy, r_in, -90, 90)
               + [(x0 + w, y0 + H - w)], fill=255)
    dc.rounded_rectangle([x0 + w, y0 + w, x0 + w + w * 0.5, y0 + H - w],
                         radius=w * 0.5, fill=255)
    return mark(tile(S), ImageChops.subtract(m, c))


# ---------------------------------------------------------------- P3 切开的 D
def p3(S, pad):
    T = S - 2 * pad
    H, W = 0.62 * T, 0.47 * T
    w = 0.235 * H
    x0, y0 = (S - W) / 2, (S - H) / 2
    cx, cy = x0 + W - H / 2, y0 + H / 2
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.polygon([(x0, y0), (cx, y0)] + arc_pts(cx, cy, H / 2, -90, 90) + [(x0, y0 + H)], fill=255)
    r_in = H / 2 - w
    c = Image.new("L", (S, S), 0)
    dc = ImageDraw.Draw(c)
    dc.polygon([(x0 + w, y0 + w), (cx, y0 + w)] + arc_pts(cx, cy, r_in, -90, 90)
               + [(x0 + w, y0 + H - w)], fill=255)
    dc.rounded_rectangle([x0 + w, y0 + w, x0 + w + w * 0.5, y0 + H - w],
                         radius=w * 0.5, fill=255)
    m = ImageChops.subtract(m, c)
    # 沿 35° 把字形切成上下两半，上半整体往左上错开
    ang = math.radians(35)
    half = Image.new("L", (S, S), 0)
    x1, x2 = -S, 2 * S
    y1 = S / 2 - (x1 - S / 2) * math.tan(ang)
    y2 = S / 2 - (x2 - S / 2) * math.tan(ang)
    ImageDraw.Draw(half).polygon([(x1, y1), (x2, y2), (x2, -S), (x1, -S)], fill=255)

    upper = ImageChops.subtract(m, ImageChops.invert(half))   # m ∩ 上半
    lower = ImageChops.subtract(m, half)                      # m ∩ 下半
    upper = shift(upper, -S * 0.020, -S * 0.020)
    m = ImageChops.lighter(upper, lower)
    return mark(tile(S), m)


# ---------------------------------------------------------------- P4 交叉胶囊 X
def p4(S, pad):
    T = S - 2 * pad
    L, t = 0.62 * T, 0.175 * T
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.polygon(capsule_poly(S / 2, S / 2, L, t, 45), fill=255)
    second = Image.new("L", (S, S), 0)
    ImageDraw.Draw(second).polygon(capsule_poly(S / 2, S / 2, L, t, -45), fill=255)
    # 在交点处留一道缝，形成"上压下穿"的编织感
    gap = Image.new("L", (S, S), 0)
    ImageDraw.Draw(gap).polygon(capsule_poly(S / 2, S / 2, t * 2.1, t * 1.02, -45), fill=255)
    second = ImageChops.subtract(second, gap)
    m = ImageChops.lighter(m, second)
    return mark(tile(S), m)


# ---------------------------------------------------------------- P5 胶囊任务栏
def p5(S, pad):
    T = S - 2 * pad
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    w, h = 0.66 * T, 0.235 * T
    x0, y0 = (S - w) / 2, (S - h) / 2
    sw = 0.062 * T
    d.rounded_rectangle([x0, y0, x0 + w, y0 + h], radius=h / 2, fill=255)
    d.rounded_rectangle([x0 + sw, y0 + sw, x0 + w - sw, y0 + h - sw],
                        radius=(h - 2 * sw) / 2, fill=0)
    slot = h - 2 * sw - 0.055 * T
    for i, k in enumerate((1.0, 1.0, 1.0)):
        sx = x0 + sw + 0.055 * T + i * (slot + 0.045 * T)
        d.rounded_rectangle([sx, y0 + h / 2 - slot / 2,
                             sx + slot, y0 + h / 2 + slot / 2],
                            radius=slot / 2, fill=255)
    return mark(tile(S), m)


# ---------------------------------------------------------------- P6 胶囊窗
def p6(S, pad):
    T = S - 2 * pad
    W, H = 0.52 * T, 0.60 * T
    x0, y0 = (S - W) / 2, (S - H) / 2
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.rounded_rectangle([x0, y0, x0 + W, y0 + H * 0.55], radius=0.12 * T, fill=255)
    d.rectangle([x0, y0 + H * 0.30, x0 + W, y0 + H * 0.62], fill=255)
    d.rounded_rectangle([x0, y0 + H * 0.42, x0 + W, y0 + H], radius=W / 2, fill=255)
    d.rectangle([x0 + W * 0.12, y0 + H * 0.63, x0 + W * 0.88, y0 + H * 0.695], fill=0)
    return mark(tile(S), m)


DIRS = [
    ("p1", "实心窗 + 负形 dock", "丢掉线框，用一个实心面块，dock 用负空间挖出来。力度最接近系统图标。", p1),
    ("p2", "D 字标（加重）", "字干加粗到 23%，一个字解决识别，缩小最稳。", p2),
    ("p3", "切开的 D", "D 被斜切一刀、上半错位，暗示「上下两层」。", p3),
    ("p4", "交叉胶囊", "两条胶囊交织成 X，取名字里的 X，也是「注入 / 交织」。", p4),
    ("p5", "胶囊任务栏", "把成品形态直接画出来：一条胶囊里放三个图标位。", p5),
    ("p6", "胶囊窗", "上半窗口、下半直接化成胶囊，一个形里完成「变形」。", p6),
]


def main():
    for key, _n, _d, fn in DIRS:
        for s in (256, 64, 32, 16):
            ss = 8 if s <= 64 else 4
            fn(s * ss, s * ss * PAD).resize((s, s), Image.LANCZOS).save(OUT / f"{key}-{s}.png")
        fn(512, 512 * PAD).save(OUT / f"{key}-512.png")
    print("sketches ->", OUT)


if __name__ == "__main__":
    main()

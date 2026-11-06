#!/usr/bin/env python3
"""DCX 图标草图 · D × X 融合 v2

v1 失败的原因：D 和 X 只是"叠"在一起，几何上没关系，所以 X 会把 D 切成碎块。
v2 改成让两者**共享端点 / 共享笔画**，四种融合逻辑：

  g1  D 环 + 织 X     细环 D 撑大碗，X 完整落在碗里
  g2  D 块 + 贯穿 X   实心 D，X 挖到边界但不切断（靠 1px 连接）
  g3  X 长成 D        X 的两左端接竖干、两右端接半圆碗，D 由 X 生长出来
  g4  X + 碗弧       只接右侧半圆，D 只留一个"碗"的暗示

    python tools/sketch_dx.py
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


def arc_pts(cx, cy, r, a0, a1, n=240):
    return [(cx + r * math.cos(math.radians(a0 + (a1 - a0) * i / n)),
             cy + r * math.sin(math.radians(a0 + (a1 - a0) * i / n))) for i in range(n + 1)]


def shift(m, dx, dy):
    return ImageChops.offset(m, int(dx), int(dy))


def _solid(S, color, mask):
    lay = Image.new("RGBA", (S, S), color)
    lay.putalpha(ImageChops.multiply(lay.split()[3], mask))
    return lay


def capsule_poly(cx, cy, L, t, deg):
    a = math.radians(deg)
    r = t / 2
    d = L / 2 - r
    c1 = (cx - d * math.cos(a), cy - d * math.sin(a))
    c2 = (cx + d * math.cos(a), cy + d * math.sin(a))
    return arc_pts(c1[0], c1[1], r, deg + 90, deg + 270) + arc_pts(c2[0], c2[1], r, deg - 90, deg + 90)


def tile(S):
    pad = S * PAD
    m = gb.tile_mask(S, pad)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(gb.gradient(S), (0, 0), m)
    rim = ImageChops.multiply(ImageChops.subtract(m, shift(m, 0, S * 0.014)), m)
    img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 58), rim))
    low = ImageChops.multiply(ImageChops.subtract(m, shift(m, 0, -S * 0.024)), m)
    low = low.filter(ImageFilter.GaussianBlur(S * 0.007))
    img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 46), low))
    return img


def mark(img, m, shadow=True):
    S = img.size[0]
    if shadow:
        sh = shift(m.filter(ImageFilter.GaussianBlur(S * 0.013)), 0, S * 0.011)
        img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 55), sh))
    return Image.alpha_composite(img, _solid(S, (255, 255, 255, 255), m))


def ring_d(S, W, H, sw):
    """D 形环：竖干 + 半圆碗，空心。"""
    x0, y0 = (S - W) / 2, (S - H) / 2
    cx, cy = x0 + W - H / 2, y0 + H / 2
    m = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(m)
    d.polygon([(x0, y0), (cx, y0)] + arc_pts(cx, cy, H / 2, -90, 90) + [(x0, y0 + H)], fill=255)
    r_in = H / 2 - sw
    c = Image.new("L", (S, S), 0)
    dc = ImageDraw.Draw(c)
    dc.polygon([(x0 + sw, y0 + sw), (cx, y0 + sw)] + arc_pts(cx, cy, r_in, -90, 90)
               + [(x0 + sw, y0 + H - sw)], fill=255)
    dc.rounded_rectangle([x0 + sw, y0 + sw, x0 + sw + sw * 0.5, y0 + H - sw],
                         radius=sw * 0.5, fill=255)
    return ImageChops.subtract(m, c)


def weave_x(S, L, t, gap=True):
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).polygon(capsule_poly(S / 2, S / 2, L, t, 45), fill=255)
    second = Image.new("L", (S, S), 0)
    ImageDraw.Draw(second).polygon(capsule_poly(S / 2, S / 2, L, t, -45), fill=255)
    if gap:
        g = Image.new("L", (S, S), 0)
        ImageDraw.Draw(g).polygon(capsule_poly(S / 2, S / 2, t * 2.05, t * 1.0, 45), fill=255)
        second = ImageChops.subtract(second, g)
    return ImageChops.lighter(m, second)


# ---------------------------------------------------------------- g1 D 环 + 织 X
def g1(S, pad):
    T = S - 2 * pad
    d = ring_d(S, 0.74 * T, 0.70 * T, 0.072 * T)
    x = weave_x(S, 0.70 * T, 0.080 * T)
    return mark(tile(S), ImageChops.lighter(d, x))


# ---------------------------------------------------------------- g5 同上，X 压一层浅色
def g5(S, pad, x_alpha=185):
    T = S - 2 * pad
    img = tile(S)
    d = ring_d(S, 0.74 * T, 0.70 * T, 0.072 * T)
    img = mark(img, d)
    x = weave_x(S, 0.70 * T, 0.080 * T)
    S_ = img.size[0]
    return Image.alpha_composite(img, _solid(S_, (255, 255, 255, x_alpha), x))


# ---------------------------------------------------------------- g2 D 块 + 贯穿 X
def g2(S, pad):
    T = S - 2 * pad
    W, H = 0.62 * T, 0.68 * T
    x0, y0 = (S - W) / 2, (S - H) / 2
    d = Image.new("L", (S, S), 0)
    ImageDraw.Draw(d).rounded_rectangle([x0, y0, x0 + W, y0 + H], radius=W * 0.34, fill=255)
    x = weave_x(S, 0.79 * T, 0.090 * T)
    return mark(tile(S), ImageChops.subtract(d, x))


# ---------------------------------------------------------------- g3 X 长成 D
def g3(S, pad):
    T = S - 2 * pad
    L, t = 0.68 * T, 0.105 * T
    cx = cy = S / 2
    half = (L / 2) * math.cos(math.radians(45))           # 端点相对中心的 x/y 偏移
    m = weave_x(S, L, t, gap=True)
    d = ImageDraw.Draw(m)
    # 竖干：连接两个左端点
    d.line([(cx - half, cy - half), (cx - half, cy + half)], fill=255, width=int(t))
    # 碗：以中心为圆心、过两个右端点的 45° 弧段
    ring = Image.new("L", (S, S), 0)
    ImageDraw.Draw(ring).arc([cx - L / 2, cy - L / 2, cx + L / 2, cy + L / 2],
                             start=-45, end=45, fill=255, width=int(t))
    m = ImageChops.lighter(m, ring)
    return mark(tile(S), m)


# ---------------------------------------------------------------- g4 X + 碗弧
def g4(S, pad):
    T = S - 2 * pad
    L, t = 0.72 * T, 0.105 * T
    cx = cy = S / 2
    m = weave_x(S, L, t, gap=True)
    ring = Image.new("L", (S, S), 0)
    ImageDraw.Draw(ring).arc([cx - L / 2, cy - L / 2, cx + L / 2, cy + L / 2],
                             start=-45, end=45, fill=255, width=int(t))
    return mark(tile(S), ImageChops.lighter(m, ring))


DIRS = [
    ("g1", "D 环 + 织 X", "细环 D 把碗撑大，X 完整落在碗里。", g1),
    ("g2", "D 块 + 贯穿 X", "实心 D，X 挖到边界但不切断，靠负空间读 X。", g2),
    ("g3", "X 长成 D", "X 两左端接竖干、两右端接半圆，D 由 X 生长出来。", g3),
    ("g4", "X + 碗弧", "只接右侧圆弧，留一个「碗」的暗示。", g4),
    ("g5", "D 环 + 浅色 X", "同 g1，X 压一层浅白，形成前后两层。", g5),
]


def main():
    for key, _n, _d, fn in DIRS:
        for s in (256, 64, 32, 16):
            ss = 8 if s <= 64 else 4
            fn(s * ss, s * ss * PAD).resize((s, s), Image.LANCZOS).save(OUT / f"{key}-{s}.png")
        fn(512, 512 * PAD).save(OUT / f"{key}-512.png")
    print("->", OUT)


if __name__ == "__main__":
    main()

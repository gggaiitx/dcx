#!/usr/bin/env python3
"""DCX 图标 · D × X 融合定稿候选（g1 / g5）

g1  D 环 + 白 X      同色同层，最干脆
g5  D 环 + 浅 X      X 压到 82% 白，形成前后两层

小于等于 20px 时去掉内部的 X，只留 D 环 —— 否则四个白区会糊成一团。

    python tools/dx_final.py
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
SMALL_MAX = 20


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


def weave_x(S, L, t):
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).polygon(capsule_poly(S / 2, S / 2, L, t, 45), fill=255)
    second = Image.new("L", (S, S), 0)
    ImageDraw.Draw(second).polygon(capsule_poly(S / 2, S / 2, L, t, -45), fill=255)
    g = Image.new("L", (S, S), 0)
    ImageDraw.Draw(g).polygon(capsule_poly(S / 2, S / 2, t * 2.05, t * 1.0, 45), fill=255)
    second = ImageChops.subtract(second, g)
    return ImageChops.lighter(m, second)


def build(S, pad, x_alpha=255, simplify=False):
    T = S - 2 * pad
    img = tile(S)
    ring = ring_d(S, 0.74 * T, 0.70 * T, 0.072 * T if not simplify else 0.090 * T)
    img = mark(img, ring)
    if not simplify:
        x = weave_x(S, 0.70 * T, 0.078 * T)
        img = Image.alpha_composite(img, _solid(S, (255, 255, 255, x_alpha), x))
    return img


def main():
    for key, alpha in (("g1", 255), ("g5", 209)):
        for s in (16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024):
            ss = 8 if s <= 64 else 4
            simplify = s <= SMALL_MAX
            build(s * ss, s * ss * PAD, x_alpha=alpha, simplify=simplify) \
                .resize((s, s), Image.LANCZOS).save(OUT / f"{key}x-{s}.png")
    print("->", OUT)


if __name__ == "__main__":
    main()

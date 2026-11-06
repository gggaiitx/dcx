#!/usr/bin/env python3
"""DCX 品牌资源生成器 · P4 交叉胶囊

标记 = 两条 45° 胶囊在中心编织成 X，其中一条在交点处让位。
所有质感由同一组参数导出：图标、托盘图标、矢量标志、锁版标志。

    python tools/gen_brand.py            # 主色（靛蓝）全套 + 收纳盒应用图标
    python tools/gen_brand.py --all      # 另加备选配色
    python tools/gen_brand.py --box-only # 只重生成 dcx-box.exe 的独立图标
"""

import math
import struct
import sys
from io import BytesIO
from pathlib import Path

from PIL import Image, ImageDraw, ImageChops, ImageFilter

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "brand"

# --------------------------------------------------------------------------
# 几何（全部为 tile 边长 T 的比例）
# --------------------------------------------------------------------------
PAD = 0.052
TILE_R = 0.228

X_L = 0.620        # 胶囊总长（端帽外缘）
X_T = 0.173        # 胶囊粗细
X_R = 1.07         # 交叉处挖掉的圆半径 / 胶囊粗细（形成月牙切口）

X_L_SM = 0.700     # ≤24px：不切缝，改成细长一些，16px 才读得出 X
X_T_SM = 0.196
X_R_SM = 1.05

SMALL_MAX = 56    # ≤56px 用实心 X：月牙在这一档会碎成两个小点
ICO_SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
PNG_SIZES = [16, 20, 24, 32, 40, 48, 64, 96, 128, 256, 512, 1024]

# --------------------------------------------------------------------------
# 色板
# --------------------------------------------------------------------------
PALETTES = {
    "indigo": {                       # 主色：靛蓝
        "name": "靛蓝 Indigo",
        "top": (0x92, 0x82, 0xFF),
        "mid": (0x50, 0x36, 0xE2),
        "bot": (0x2C, 0x1B, 0xB4),
        "accent": "#5B45E8",
        "bloom": (0xBF, 0xB4, 0xFF),
    },
    "graphite": {                     # 备选：石墨
        "name": "石墨 Graphite",
        "top": (0x6B, 0x76, 0x88),
        "mid": (0x3D, 0x46, 0x56),
        "bot": (0x22, 0x28, 0x33),
        "accent": "#48525F",
        "bloom": (0x9F, 0xB4, 0xCC),
    },
    "teal": {                         # 主色：深青
        "name": "深青 Teal",
        "top": (0x1F, 0x94, 0x90),
        "mid": (0x0A, 0x6E, 0x72),
        "bot": (0x04, 0x36, 0x3F),
        "accent": "#0E7C7E",
        "bloom": (0x9C, 0xF6, 0xE6),
    },
}
MAIN = "teal"     # 主色：深青（其余为备选演示）

# 标记自身的明暗：顶部纯白，底部压成冷灰，让"管"有体积
MARK_HI = (0xFF, 0xFF, 0xFF)
MARK_LO = (0xF3, 0xF5, 0xFC)


def lerp(a, b, t):
    return tuple(int(round(a[i] + (b[i] - a[i]) * t)) for i in range(3))


def _field(S, fn, res=256):
    """把 (u,v)∈[0,1]² 的解析场烘成 L 图；够平滑，可安全放大。"""
    small = Image.new("L", (res, res))
    px = small.load()
    for j in range(res):
        v = (j + 0.5) / res
        for i in range(res):
            u = (i + 0.5) / res
            px[i, j] = max(0, min(255, int(round(fn(u, v) * 255))))
    return small.resize((S, S), Image.BICUBIC)


def _blur(mask, r):
    if r < 0.6:
        return mask
    return mask.filter(ImageFilter.GaussianBlur(r))


def _solid(S, color, mask):
    lay = Image.new("RGBA", (S, S), tuple(color))
    lay.putalpha(mask)
    return lay


# --------------------------------------------------------------------------
# 胶囊几何
# --------------------------------------------------------------------------
def arc_pts(cx, cy, r, a0, a1, n=240):
    return [(cx + r * math.cos(math.radians(a0 + (a1 - a0) * i / n)),
             cy + r * math.sin(math.radians(a0 + (a1 - a0) * i / n))) for i in range(n + 1)]


def capsule_poly(cx, cy, L, t, deg):
    a = math.radians(deg)
    r = t / 2
    d = L / 2 - r
    c1 = (cx - d * math.cos(a), cy - d * math.sin(a))
    c2 = (cx + d * math.cos(a), cy + d * math.sin(a))
    return arc_pts(c1[0], c1[1], r, deg + 90, deg + 270) + arc_pts(c2[0], c2[1], r, deg - 90, deg + 90)


def capsule_mask(S, L, t, deg):
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).polygon(capsule_poly(S / 2, S / 2, L, t, deg), fill=255)
    return m


def shift(m, dx, dy):
    return ImageChops.offset(m, int(round(dx)), int(round(dy)))


# --------------------------------------------------------------------------
# 图标块
# --------------------------------------------------------------------------
def tile_gradient(S, pal, diagonal=True):
    top, mid, bot = pal["top"], pal["mid"], pal["bot"]
    if diagonal:
        lh = Image.linear_gradient("L").rotate(-90, expand=True).resize((S, S))
        lv = Image.linear_gradient("L").resize((S, S))
        diag = ImageChops.add(lh, lv, scale=2.0)
    else:
        diag = Image.linear_gradient("L").resize((S, S))
    stops = [lerp(top, mid, i / 127.0) if i < 128 else lerp(mid, bot, (i - 128) / 127.0)
             for i in range(256)]
    return Image.merge("RGB", (diag.point(lambda v: stops[v][0]),
                               diag.point(lambda v: stops[v][1]),
                               diag.point(lambda v: stops[v][2])))


def tile_mask(S, pad):
    inner = S - 2 * pad
    m = Image.new("L", (S, S), 0)
    ImageDraw.Draw(m).rounded_rectangle([pad, pad, pad + inner, pad + inner],
                                        radius=max(1, inner * TILE_R), fill=255)
    return m


def tile_layer(S, pad, pal, detail=True):
    m = tile_mask(S, pad)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(tile_gradient(S, pal), (0, 0), m)

    if detail:
        # 左上柔光：让表面像有光源而不是纯色块
        def _bloom(u, v):
            d = math.hypot(u - 0.30, v - 0.22) / 0.86
            return max(0.0, 1.0 - d) ** 2.1
        img = Image.alpha_composite(img, _solid(S, pal["bloom"] + (0,),
                                                ImageChops.multiply(_field(S, _bloom).point(lambda x: x * 0.38), m)))

        # 底部下沿加深，做出厚度
        img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 0),
                                                ImageChops.multiply(_field(S, lambda u, v: v ** 2.4).point(lambda x: x * 0.19), m)))

    # 内缘上高光 / 内缘下阴影
    rim = ImageChops.subtract(m, shift(m, 0, S * 0.010))
    rim = _blur(rim, S * 0.0022)
    img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 0), rim.point(lambda v: v * 0.34)))

    low = ImageChops.subtract(m, shift(m, 0, -S * 0.018))
    low = _blur(low, S * 0.006)
    img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 0), low.point(lambda v: v * 0.22)))

    if detail and S >= 512:
        n = Image.effect_noise((S, S), 14).convert("L")
        img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 0),
                                                ImageChops.multiply(n.point(lambda v: 7), m)))
    return img


# --------------------------------------------------------------------------
# 标记：两根有体积的胶囊
# --------------------------------------------------------------------------
def tube_color(S, mask, deg, t, hi=MARK_HI, lo=MARK_LO):
    """沿胶囊法线做明暗：受光面朝左上，暗面朝右下。"""
    a = math.radians(deg)
    px, py = -math.sin(a), math.cos(a)          # 法线
    if px + py > 0:                             # 统一取"朝左上"那一侧为亮面
        px, py = -px, -py

    def _shade(u, v):
        proj = ((u - 0.5) * S * px + (v - 0.5) * S * py)
        s = min(1.0, max(0.0, 0.5 + proj / max(1e-3, t)))
        return s ** 0.85

    shade = _field(S, _shade)
    ch = []
    for i in range(3):
        a0, a1 = hi[i], lo[i]
        ch.append(shade.point(lambda v, a0=a0, a1=a1: int(round(a0 + (a1 - a0) * v / 255))))
    out = Image.merge("RGB", ch).convert("RGBA")
    out.putalpha(mask)
    return out


def specular(S, mask, deg, strength):
    """管身上沿的一道细高光，靠法线方向位移取差。"""
    a = math.radians(deg)
    px, py = -math.sin(a), math.cos(a)
    if px + py > 0:
        px, py = -px, -py
    d = S * 0.016
    band = ImageChops.subtract(mask, shift(mask, px * d, py * d))
    band = _blur(band, S * 0.0022)
    return band.point(lambda v: int(v * strength))


def mark_layers(S, pad, simple=False):
    """返回 (底层胶囊, 上层胶囊, 断口) 三个 mask，供分层合成。"""
    T = S - 2 * pad
    L = (X_L_SM if simple else X_L) * T
    t = (X_T_SM if simple else X_T) * T
    gap_k = X_R_SM if simple else X_R

    over = capsule_mask(S, L, t, 45)                  # 压在上面的一条
    under = capsule_mask(S, L, t, -45)                # 被压的一条
    gap = None
    if not simple:
        gap = Image.new("L", (S, S), 0)
        rr = t * gap_k
        ImageDraw.Draw(gap).ellipse([S / 2 - rr, S / 2 - rr, S / 2 + rr, S / 2 + rr], fill=255)
        under = ImageChops.subtract(under, gap)
    return under, over, t, gap


def render_mark(S, pad, simple=False, detail=True):
    """透明底的标记，供托盘 / 单色场景使用。"""
    under, over, t, gap = mark_layers(S, pad, simple)
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img = Image.alpha_composite(img, tube_color(S, under, -45, t))
    img = Image.alpha_composite(img, tube_color(S, over, 45, t))
    if detail:
        img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 0),
                                                specular(S, over, 45, 0.16)))
        img = Image.alpha_composite(img, _solid(S, (255, 255, 255, 0),
                                                specular(S, under, -45, 0.10)))
    return img


def render_icon(size, pal_key=MAIN, detail=True):
    ss = 8 if size <= 48 else (4 if size <= 128 else 2)
    S = size * ss
    pad = S * PAD
    pal = PALETTES[pal_key]
    simple = size <= SMALL_MAX

    img = tile_layer(S, pad, pal, detail=detail and S >= 96)
    m = render_mark(S, pad, simple, detail=detail and S >= 48)
    if detail:
        sh = shift(m.split()[3], S * 0.003, S * 0.012)
        sh = sh.filter(ImageFilter.GaussianBlur(S * 0.011)).point(lambda v: v * 0.34)
        img = Image.alpha_composite(img, _solid(S, (0, 0, 0, 0), sh))
    img = Image.alpha_composite(img, m)
    return img.convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")


# 托盘用深青的两档：深色任务栏上整体提亮一档，否则整块会沉进背景
TRAY_PAL = {
    "light": PALETTES["teal"],
    "dark": {
        "name": "深青·亮档",
        "top": (0x2E, 0xB4, 0xAB),
        "mid": (0x14, 0x84, 0x88),
        "bot": (0x06, 0x46, 0x52),
        "accent": "#0E7C7E",
        "bloom": (0xB4, 0xFF, 0xF2),
    },
}


def render_tray(size, on_dark):
    """通知区图标 = 缩小版的 app 图标：深青圆角块 + 白色实心 X。

    托盘实际只有 16–24 物理像素：编织切口在这一档必然退化成"缺角"，
    而裸 X 又会被读成关闭/错误按钮（Windows 自己就用叉表示"无图标"）。
    所以这里不画裸字形，改成带品牌底色的"块"——16px 仍是一个完整方块，
    与任务栏里其它彩色图标同一套语言。
    """
    ss = 8 if size <= 64 else 4
    S = size * ss
    pad = S * 0.045
    pal = TRAY_PAL["dark" if on_dark else "light"]

    # 底：斜向渐变 + 圆角
    img = Image.new("RGBA", (S, S), (0, 0, 0, 0))
    img.paste(tile_gradient(S, pal), (0, 0), tile_mask(S, pad))

    # 字：实心 X（不切缝），白色 —— 小尺寸下靠对比度而不是细节
    Tt = S - 2 * (pad * 1.95)
    L, t = 0.66 * Tt, 0.185 * Tt
    m = ImageChops.lighter(capsule_mask(S, L, t, 45), capsule_mask(S, L, t, -45))
    ink = Image.new("RGBA", (S, S), (255, 255, 255, 255))
    ink.putalpha(m)
    img = Image.alpha_composite(img, ink)
    return img.convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")


# --------------------------------------------------------------------------
# 独立应用图标：桌面收纳盒（dcx-box.exe）
#
# 与平台主图标（交叉胶囊 X）刻意不同：靛蓝底 + 白色「带盖、分格的盒子」。
# 同一套圆角色块保持家族感，形状一眼可分；16 px 下只保留盖与盒身外框，
# 中间的分类竖线在这一档会糊成一团，故省略。
# --------------------------------------------------------------------------
BOX_PAL = "indigo"


def render_box_icon(size, pal_key=BOX_PAL, detail=True):
    ss = 8 if size <= 48 else (4 if size <= 128 else 2)
    S = size * ss
    pad = S * PAD
    pal = PALETTES[pal_key]

    img = tile_layer(S, pad, pal, detail=detail and S >= 96)

    f = lambda v: int(round(v * S))
    mask = Image.new("L", (S, S), 0)
    d = ImageDraw.Draw(mask)
    # 盒盖：略宽于盒身，读起来像「合上的收纳盒」
    d.rounded_rectangle(
        (f(0.19), f(0.285), f(0.81), f(0.385)), radius=f(0.028), fill=255
    )
    # 盒身：白色描边（外框填白、内框挖空），中间竖线表示分类格
    t = f(0.055)
    d.rounded_rectangle(
        (f(0.245), f(0.425), f(0.755), f(0.735)), radius=f(0.045), fill=255
    )
    d.rounded_rectangle(
        (f(0.245) + t, f(0.425) + t, f(0.755) - t, f(0.735) - t),
        radius=max(1, f(0.020)),
        fill=0,
    )
    if size > 24:
        d.rectangle((f(0.482), f(0.470), f(0.518), f(0.690)), fill=255)

    ink = Image.new("RGBA", (S, S), (255, 255, 255, 255))
    ink.putalpha(mask)
    img = Image.alpha_composite(img, ink)
    return img.convert("RGBa").resize((size, size), Image.LANCZOS).convert("RGBA")


def generate_box():
    """写出 dcx-box.exe 需要的全部图标产物；不触碰平台主图标的文件。"""
    (OUT / "icons").mkdir(parents=True, exist_ok=True)
    (OUT / "embed").mkdir(parents=True, exist_ok=True)
    frames = [(s, render_box_icon(s)) for s in ICO_SIZES]
    for s, im in frames:
        im.save(OUT / "icons" / f"box-{s}.png")
    write_ico(OUT / "box.ico", frames)
    for s in (32, 64, 256):
        (OUT / "embed" / f"box-{s}.rgba").write_bytes(render_box_icon(s).tobytes())
    print("box app assets ->", OUT)


# --------------------------------------------------------------------------
# ICO
# --------------------------------------------------------------------------
def _dib(img):
    w, h = img.size
    px = img.convert("RGBA").load()
    rows = []
    for y in range(h - 1, -1, -1):
        row = bytearray()
        for x in range(w):
            r, g, b, a = px[x, y]
            row += bytes((b, g, r, a))
        rows.append(bytes(row))
    xor = b"".join(rows)
    row_bytes = ((w + 31) // 32) * 4
    mask_rows = []
    for y in range(h - 1, -1, -1):
        bits = bytearray(row_bytes)
        for x in range(w):
            if px[x, y][3] < 128:
                bits[x // 8] |= 0x80 >> (x % 8)
        mask_rows.append(bytes(bits))
    mask = b"".join(mask_rows)
    return struct.pack("<IiiHHIIiiII", 40, w, h * 2, 1, 32, 0, len(xor), 0, 0, 0, 0) + xor + mask


def _png(img):
    buf = BytesIO()
    img.save(buf, format="PNG", optimize=True)
    return buf.getvalue()


def write_ico(path, frames):
    blobs = [(s, _dib(im) if s < 128 else _png(im)) for s, im in frames]
    header = struct.pack("<HHH", 0, 1, len(blobs))
    offset = 6 + 16 * len(blobs)
    entries, data = b"", b""
    for size, blob in blobs:
        entries += struct.pack("<BBBBHHII", size % 256, size % 256, 0, 0, 1, 32,
                               len(blob), offset + len(data))
        data += blob
    path.write_bytes(header + entries + data)


# --------------------------------------------------------------------------
# 矢量
# --------------------------------------------------------------------------
VB = 256.0


def _f(v):
    return f"{v:.2f}".rstrip("0").rstrip(".")


def _hex(c):
    return "#%02X%02X%02X" % c


def svg_mark(pal_key=MAIN, mono=False, with_tile=True):
    pal = PALETTES[pal_key]
    pad = VB * PAD
    T = VB - 2 * pad
    L, t = X_L * T, X_T * T
    g = t * X_R * 2.0
    half = T * TILE_R
    fill = "currentColor" if mono else "url(#tube)"

    if mono or not with_tile:
        tile_defs, tile = "", ""
    else:
        tile_defs = f"""  <defs>
    <linearGradient id="tile" gradientUnits="userSpaceOnUse" x1="0" y1="0" x2="{_f(VB)}" y2="{_f(VB)}">
      <stop offset="0" stop-color="{_hex(pal['top'])}"/>
      <stop offset=".5" stop-color="{_hex(pal['mid'])}"/>
      <stop offset="1" stop-color="{_hex(pal['bot'])}"/>
    </linearGradient>
    <radialGradient id="bloom" gradientUnits="userSpaceOnUse" cx="{_f(VB*0.30)}" cy="{_f(VB*0.22)}" r="{_f(VB*0.86)}">
      <stop offset="0" stop-color="{_hex(pal['bloom'])}" stop-opacity=".38"/>
      <stop offset="1" stop-color="{_hex(pal['bloom'])}" stop-opacity="0"/>
    </radialGradient>
    <linearGradient id="depth" gradientUnits="userSpaceOnUse" x1="0" y1="{_f(pad)}" x2="0" y2="{_f(pad+T)}">
      <stop offset=".55" stop-color="#000" stop-opacity="0"/>
      <stop offset="1" stop-color="#000" stop-opacity=".30"/>
    </linearGradient>
    <linearGradient id="tubeA" gradientUnits="userSpaceOnUse"
        x1="{_f(VB/2 + L*0.36)}" y1="{_f(VB/2 + L*0.36)}"
        x2="{_f(VB/2 - L*0.36)}" y2="{_f(VB/2 - L*0.36)}">
      <stop offset="0" stop-color="{_hex(MARK_LO)}"/>
      <stop offset=".55" stop-color="#FFFFFF"/>
      <stop offset="1" stop-color="#FFFFFF"/>
    </linearGradient>
    <linearGradient id="tubeB" gradientUnits="userSpaceOnUse"
        x1="{_f(VB/2 + L*0.36)}" y1="{_f(VB/2 - L*0.36)}"
        x2="{_f(VB/2 - L*0.36)}" y2="{_f(VB/2 + L*0.36)}">
      <stop offset="0" stop-color="{_hex(MARK_LO)}"/>
      <stop offset=".55" stop-color="#FFFFFF"/>
      <stop offset="1" stop-color="#FFFFFF"/>
    </linearGradient>
    <mask id="weave">
      <rect width="{_f(VB)}" height="{_f(VB)}" fill="#fff"/>
      <circle cx="{_f(VB/2)}" cy="{_f(VB/2)}" r="{_f(t * X_R)}" fill="#000"/>
    </mask>
    <filter id="msoft" x="-30%" y="-30%" width="160%" height="160%">
      <feGaussianBlur stdDeviation="{_f(t*0.26)}"/>
    </filter>
  </defs>
"""
        tile = (f'  <g clip-path="url(#tclip)">\n'
                f'    <rect x="{_f(pad)}" y="{_f(pad)}" width="{_f(T)}" height="{_f(T)}" rx="{_f(half)}" fill="url(#tile)"/>\n'
                f'    <rect x="{_f(pad)}" y="{_f(pad)}" width="{_f(T)}" height="{_f(T)}" rx="{_f(half)}" fill="url(#bloom)"/>\n'
                f'    <rect x="{_f(pad)}" y="{_f(pad)}" width="{_f(T)}" height="{_f(T)}" rx="{_f(half)}" fill="url(#depth)"/>\n'
                f'  </g>\n')

    def cap(angle, fillref, mask=None):
        m = f' mask="url(#{mask})"' if mask else ""
        return (f'    <g{m} transform="rotate({angle} {_f(VB/2)} {_f(VB/2)})">'
                f'<rect x="{_f(VB/2 - L/2)}" y="{_f(VB/2 - t/2)}" width="{_f(L)}" height="{_f(t)}" '
                f'rx="{_f(t/2)}" fill="{fillref}"/></g>\n')

    shadow = ""
    if not mono and with_tile:
        shadow = (f'  <g filter="url(#msoft)" opacity=".20" '
                  f'transform="translate(0 {_f(t * 0.10)})">'
                  f'{cap(45, "#100C24")}{cap(-45, "#100C24", "weave")}</g>\n')

    clip = f'  <clipPath id="tclip"><rect x="{_f(pad)}" y="{_f(pad)}" width="{_f(T)}" height="{_f(T)}" rx="{_f(half)}"/></clipPath>\n'
    body = (f'{cap(-45, "url(#tubeB)" if not mono else fill, "weave")}'
            f'{cap(45, "url(#tubeA)" if not mono else fill)}')

    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {_f(VB)} {_f(VB)}" '
            f'role="img" aria-label="DCX">\n{tile_defs}{clip}{tile}{shadow}{body}</svg>\n')


# 字标：几何等线，cap height 100、笔画 13.5
WM_SW = 14.6
WM_CAP = 100.0
WM_D_W = 76.0
WM_C_W = 92.0
WM_X_W = 88.0
WM_GAP = [34.0, 28.0]
WM_W = WM_D_W + WM_GAP[0] + WM_C_W + WM_GAP[1] + WM_X_W


def wordmark_paths():
    sw, cap = WM_SW, WM_CAP
    h = sw / 2
    d_cx = WM_D_W - cap / 2
    d_path = (f"M {h:.2f} {h:.2f} H {d_cx:.2f} "
              f"A {cap/2 - h:.2f} {cap/2 - h:.2f} 0 0 1 {d_cx:.2f} {cap - h:.2f} "
              f"H {h:.2f} Z")
    cx = WM_D_W + WM_GAP[0] + WM_C_W / 2
    rr = cap / 2 - h
    a = math.radians(22)
    p1 = (cx + rr * math.cos(-a), cap / 2 + rr * math.sin(-a))
    p2 = (cx + rr * math.cos(a), cap / 2 + rr * math.sin(a))
    c_path = f"M {p1[0]:.2f} {p1[1]:.2f} A {rr:.2f} {rr:.2f} 0 1 0 {p2[0]:.2f} {p2[1]:.2f}"
    x0 = WM_D_W + WM_GAP[0] + WM_C_W + WM_GAP[1]
    x_path = (f"M {x0 + h:.2f} {h:.2f} L {x0 + WM_X_W - h:.2f} {cap - h:.2f} "
              f"M {x0 + WM_X_W - h:.2f} {h:.2f} L {x0 + h:.2f} {cap - h:.2f}")
    return d_path, c_path, x_path


def svg_wordmark(color="#1A1A1A"):
    d, c, x = wordmark_paths()
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {WM_W:.2f} {WM_CAP:.2f}" '
            f'role="img" aria-label="DCX">\n'
            f'  <g fill="none" stroke="{color}" stroke-width="{WM_SW}" stroke-linejoin="round">\n'
            f'    <path d="{d}"/>\n    <path d="{c}"/>\n    <path d="{x}"/>\n'
            f'  </g>\n</svg>\n')


# --------------------------------------------------------------------------
def main():
    if "--box-only" in sys.argv:
        generate_box()
        return

    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "icons").mkdir(exist_ok=True)
    (OUT / "tray").mkdir(exist_ok=True)

    frames = [(s, render_icon(s)) for s in ICO_SIZES]
    for s, im in frames:
        im.save(OUT / "icons" / f"dcx-{s}.png")
    for s in PNG_SIZES:
        if s not in ICO_SIZES:
            render_icon(s).save(OUT / "icons" / f"dcx-{s}.png")
    write_ico(OUT / "dcx.ico", frames)

    for s in (16, 20, 24, 32, 48):
        render_tray(s, True).save(OUT / "tray" / f"tray-{s}-on-dark.png")
        render_tray(s, False).save(OUT / "tray" / f"tray-{s}-on-light.png")
    render_tray(32, False).save(OUT / "tray" / "tray-32.png")

    # 裸 RGBA（无文件头），供 Rust 侧 include_bytes! + Icon::from_rgba 使用。
    # 这样宿主不必依赖 PNG 解码器，托盘/窗口图标也就能随二进制一起分发。
    emb = OUT / "embed"
    emb.mkdir(exist_ok=True)
    for s in (16, 20, 24, 32):
        (emb / f"tray-{s}-on-dark.rgba").write_bytes(render_tray(s, True).tobytes())
        (emb / f"tray-{s}-on-light.rgba").write_bytes(render_tray(s, False).tobytes())
    for s in (32, 64, 256):
        (emb / f"icon-{s}.rgba").write_bytes(render_icon(s).tobytes())

    (OUT / "mark.svg").write_text(svg_mark(), encoding="utf-8")
    (OUT / "mark-mono.svg").write_text(svg_mark(mono=True), encoding="utf-8")
    (OUT / "wordmark.svg").write_text(svg_wordmark(), encoding="utf-8")
    (OUT / "wordmark-dark.svg").write_text(svg_wordmark("#FFFFFF"), encoding="utf-8")

    if "--all" in sys.argv:
        (OUT / "colors").mkdir(exist_ok=True)
        for key in PALETTES:
            for s in (64, 128, 256):
                render_icon(s, key).save(OUT / "colors" / f"{key}-{s}.png")
            (OUT / "colors" / f"{key}.svg").write_text(svg_mark(key), encoding="utf-8")

    generate_box()
    print("brand assets ->", OUT)


if __name__ == "__main__":
    main()

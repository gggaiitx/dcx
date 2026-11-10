"""Generate the desktop-snapshot application's icon resources.

Original artwork for DCX: a viewfinder (four detached corner brackets) around a
shutter dot, on the same rounded tile the other DCX app icons use. Nothing is
taken from another icon set — the shape is described analytically here and
rasterised by a small SDF, exactly like ``gen_search_icon.py`` does.

Outputs:

* ``brand/embed/snip-64.rgba`` — raw 64x64 RGBA for ``include_bytes!``.
* ``brand/snip.ico`` — multi-size ICO (16 … 256, PNG entries) for ``build.rs``.
* ``brand/icons/snip-<size>.png`` — preview / documentation sizes.

Standard library only: PNG and ICO containers are written with ``struct`` and
``zlib``; the drawing is an anti-aliased signed-distance rasteriser.

Run from anywhere: paths resolve relative to this file.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT_RGBA = ROOT / "brand" / "embed" / "snip-64.rgba"
OUT_ICO = ROOT / "brand" / "snip.ico"
OUT_PNG_DIR = ROOT / "brand" / "icons"

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

# Cool teal, one step bluer than the search icon so the two are not confused
# at 16 px in the taskbar.
TEAL_TOP = (0x2F, 0xB3, 0x9E)
TEAL_BOTTOM = (0x0A, 0x4F, 0x4A)
WHITE = (0xFF, 0xFF, 0xFF)


def smoothstep(edge0: float, edge1: float, x: float) -> float:
    t = max(0.0, min(1.0, (x - edge0) / (edge1 - edge0)))
    return t * t * (3.0 - 2.0 * t)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def sd_round_rect(px: float, py: float, cx: float, cy: float,
                  hw: float, hh: float, r: float) -> float:
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ax, ay = max(qx, 0.0), max(qy, 0.0)
    return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - r


def rect_cov(px: float, py: float, cx: float, cy: float,
             hw: float, hh: float, r: float) -> float:
    return 1.0 - smoothstep(-0.75, 0.75, sd_round_rect(px, py, cx, cy, hw, hh, r))


def capsule_cov(px: float, py: float, x0: float, y0: float,
                x1: float, y1: float, half_w: float) -> float:
    dx, dy = x1 - x0, y1 - y0
    length_sq = dx * dx + dy * dy
    t = 0.0 if length_sq == 0 else max(
        0.0, min(1.0, ((px - x0) * dx + (py - y0) * dy) / length_sq)
    )
    qx, qy = x0 + t * dx, y0 + t * dy
    d = math.hypot(px - qx, py - qy)
    return 1.0 - smoothstep(half_w - 0.75, half_w + 0.75, d)


def dot_cov(px: float, py: float, cx: float, cy: float, r: float) -> float:
    d = math.hypot(px - cx, py - cy)
    return 1.0 - smoothstep(r - 0.75, r + 0.75, d)


def render(size: int) -> bytes:
    out = bytearray(size * size * 4)
    radius = 0.205 * size
    # Bracket geometry: an inner frame at 0.20…0.80 of the tile, with arms a
    # little over a fifth of the frame so the gap between corners stays wide.
    lo, hi = 0.205 * size, 0.795 * size
    arm = 0.20 * (hi - lo)
    stroke = max(1.35, 0.078 * size)
    if size <= 24:
        stroke = max(1.6, 0.098 * size)
    half_w = stroke * 0.5
    dot_r = max(1.5, 0.075 * size)

    for y in range(size):
        for x in range(size):
            px, py = x + 0.5, y + 0.5
            tile = rect_cov(px, py, size / 2, size / 2, size / 2, size / 2, radius)
            if tile <= 0.0:
                continue
            t = y / max(1, size - 1)
            r = lerp(TEAL_TOP[0], TEAL_BOTTOM[0], t)
            g = lerp(TEAL_TOP[1], TEAL_BOTTOM[1], t)
            b = lerp(TEAL_TOP[2], TEAL_BOTTOM[2], t)
            # Soft bloom from the upper-left, same lighting direction as the
            # other DCX tiles.
            bloom = (1.0 - min(
                1.0,
                math.hypot(px - size * 0.24, py - size * 0.16) / (size * 0.62),
            )) * 0.11
            r = lerp(r, WHITE[0], bloom)
            g = lerp(g, WHITE[1], bloom)
            b = lerp(b, WHITE[2], bloom)

            glyph = 0.0
            for cx, cy, sx, sy in (
                (lo, lo, 1.0, 1.0),
                (hi, lo, -1.0, 1.0),
                (lo, hi, 1.0, -1.0),
                (hi, hi, -1.0, -1.0),
            ):
                glyph = max(
                    glyph,
                    capsule_cov(px, py, cx, cy, cx + arm * sx, cy, half_w),
                    capsule_cov(px, py, cx, cy, cx, cy + arm * sy, half_w),
                )
            glyph = max(glyph, dot_cov(px, py, size * 0.5, size * 0.52, dot_r))

            if glyph > 0.0:
                r = lerp(r, WHITE[0], glyph)
                g = lerp(g, WHITE[1], glyph)
                b = lerp(b, WHITE[2], glyph)

            i = (y * size + x) * 4
            out[i] = round(r)
            out[i + 1] = round(g)
            out[i + 2] = round(b)
            out[i + 3] = round(tile * 255)
    return bytes(out)


def png_from_rgba(rgba: bytes, size: int) -> bytes:
    stride = size * 4
    raw = b"".join(b"\x00" + rgba[y * stride:(y + 1) * stride] for y in range(size))

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )


def ico_from_pngs(items: list[tuple[int, bytes]]) -> bytes:
    header = struct.pack("<HHH", 0, 1, len(items))
    offset = 6 + 16 * len(items)
    entries = []
    for size, data in items:
        dim = 0 if size >= 256 else size
        entries.append(
            struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        )
        offset += len(data)
    return header + b"".join(entries) + b"".join(data for _, data in items)


def main() -> None:
    OUT_RGBA.parent.mkdir(parents=True, exist_ok=True)
    OUT_ICO.parent.mkdir(parents=True, exist_ok=True)
    OUT_PNG_DIR.mkdir(parents=True, exist_ok=True)
    OUT_RGBA.write_bytes(render(64))
    items = [(size, png_from_rgba(render(size), size)) for size in SIZES]
    OUT_ICO.write_bytes(ico_from_pngs(items))
    for size in SIZES:
        (OUT_PNG_DIR / f"snip-{size}.png").write_bytes(png_from_rgba(render(size), size))
    print(f"wrote {OUT_RGBA} (16384 bytes)")
    print(f"wrote {OUT_ICO} ({OUT_ICO.stat().st_size} bytes)")
    print(f"wrote {len(SIZES)} png previews into {OUT_PNG_DIR}")


if __name__ == "__main__":
    main()

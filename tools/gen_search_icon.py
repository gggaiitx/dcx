"""Generate the file-search application's icon resources.

The 2026-11 redesign puts the magnifier on the same teal rounded tile as the
DCX brand mark (``brand/icons/dcx-64.png``) instead of leaving it floating on a
transparent background. Produces two byte-exact artefacts used by
``crates/dcx-search``:

* ``brand/embed/search-64.rgba`` — raw 64x64 RGBA, embedded by ``ui.rs``
  via ``include_bytes!`` for the egui window icon.
* ``brand/search.ico`` — a multi-size ICO (16 … 256, PNG entries) for
  ``build.rs`` to place in the PE resource section (Explorer / Alt-Tab).

No third-party modules: PNG and ICO containers are written with ``struct``
and ``zlib`` from the standard library; the drawing is an analytic, anti-aliased
SDF rasteriser (rounded tile, ring, capsule handle, small text bars).

Run from anywhere: paths are resolved relative to this file.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT_RGBA = ROOT / "brand" / "embed" / "search-64.rgba"
OUT_ICO = ROOT / "brand" / "search.ico"

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]
TEAL_TOP = (0x3A, 0xA4, 0xA1)
TEAL_BOTTOM = (0x08, 0x52, 0x55)
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


def ring_cov(px: float, py: float, cx: float, cy: float,
             r_outer: float, w: float) -> float:
    d = math.hypot(px - cx, py - cy)
    return smoothstep(r_outer - w - 0.75, r_outer - w + 0.2, d) * (
        1.0 - smoothstep(r_outer - 0.2, r_outer + 0.75, d)
    )


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


def bar_cov(px: float, py: float, cx: float, cy: float,
            hw: float, hh: float) -> float:
    dx = max(abs(px - cx) - (hw - hh), 0.0)
    dy = abs(py - cy)
    return 1.0 - smoothstep(hh - 0.6, hh + 0.6, math.hypot(dx, max(dy, 0.0)))


def render(size: int) -> bytes:
    out = bytearray(size * size * 4)
    radius = 0.205 * size
    cx, cy = 0.425 * size, 0.415 * size
    ring_r = 0.225 * size
    stroke = max(1.45, 0.086 * size)
    if size <= 24:
        stroke = max(1.5, 0.105 * size)
    half_w = max(1.2, stroke * 0.52)
    start_r = ring_r - stroke * 0.35
    hx0 = cx + start_r * 0.7071
    hy0 = cy + start_r * 0.7071
    hx1, hy1 = 0.775 * size, 0.775 * size
    show_text = size >= 48

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
            bloom = (1.0 - min(
                1.0,
                math.hypot(px - size * 0.25, py - size * 0.18) / (size * 0.6),
            )) * 0.10
            r = lerp(r, WHITE[0], bloom)
            g = lerp(g, WHITE[1], bloom)
            b = lerp(b, WHITE[2], bloom)

            glyph = max(
                ring_cov(px, py, cx, cy, ring_r, stroke),
                capsule_cov(px, py, hx0, hy0, hx1, hy1, half_w),
            )
            if show_text:
                bar1 = bar_cov(px, py, cx, cy - 0.045 * size,
                               0.090 * size, max(0.9, 0.014 * size))
                bar2 = bar_cov(px, py, cx, cy + 0.028 * size,
                               0.060 * size, max(0.9, 0.014 * size))
                glyph = max(glyph, min(1.0, (bar1 + bar2) * 0.85))
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
    # ICONDIR + one ICONDIRENTRY per size + PNG payloads. Windows accepts
    # PNG-compressed entries for 32 bpp icons; width/height 0 means 256.
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
    OUT_RGBA.write_bytes(render(64))
    items = [(size, png_from_rgba(render(size), size)) for size in SIZES]
    OUT_ICO.write_bytes(ico_from_pngs(items))
    print(f"wrote {OUT_RGBA} (16384 bytes)")
    print(f"wrote {OUT_ICO} ({OUT_ICO.stat().st_size} bytes)")


if __name__ == "__main__":
    main()

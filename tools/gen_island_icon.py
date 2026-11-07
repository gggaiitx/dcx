"""Generate the desktop-island application's icon resources.

Same technique as ``tools/gen_search_icon.py``: an analytic, anti-aliased SDF
rasteriser and hand-written PNG/ICO containers, so no third-party module is
needed. Produces the two artefacts ``crates/dcx-island`` embeds:

* ``brand/embed/island-64.rgba`` — raw 64x64 RGBA for the egui window icon.
* ``brand/island.ico`` — multi-size ICO (PNG entries) for the PE resource
  section, i.e. Explorer / Alt-Tab / the settings centre.

The mark is a capsule (the island itself) on the DCX indigo tile, with
two indicator dots: time on the left, an activity bar on the right.

Run from anywhere; paths resolve relative to this file.
"""

from __future__ import annotations

import math
import struct
import zlib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
OUT_RGBA = ROOT / "brand" / "embed" / "island-64.rgba"
OUT_ICO = ROOT / "brand" / "island.ico"

SIZES = [16, 20, 24, 32, 40, 48, 64, 128, 256]

INDIGO_TOP = (0x92, 0x82, 0xFF)
INDIGO_BOTTOM = (0x3A, 0x22, 0xC8)
WHITE = (0xFF, 0xFF, 0xFF)
ACCENT = (0x8F, 0x7B, 0xFF)


def smoothstep(edge0: float, edge1: float, x: float) -> float:
    if edge1 == edge0:
        return 0.0 if x < edge0 else 1.0
    t = max(0.0, min(1.0, (x - edge0) / (edge1 - edge0)))
    return t * t * (3.0 - 2.0 * t)


def lerp(a: float, b: float, t: float) -> float:
    return a + (b - a) * t


def sd_round_rect(px, py, cx, cy, hw, hh, r) -> float:
    qx = abs(px - cx) - (hw - r)
    qy = abs(py - cy) - (hh - r)
    ax, ay = max(qx, 0.0), max(qy, 0.0)
    return math.hypot(ax, ay) + min(max(qx, qy), 0.0) - r


def cov(px, py, cx, cy, hw, hh, r) -> float:
    return 1.0 - smoothstep(-0.75, 0.75, sd_round_rect(px, py, cx, cy, hw, hh, r))


def mix(dst, src, a):
    return tuple(dst[i] + (src[i] - dst[i]) * a for i in range(3))


def render(size: int) -> bytearray:
    """RGBA buffer, straight (non-premultiplied) alpha, top-down."""
    buf = bytearray(size * size * 4)
    s = float(size)
    # Relative geometry; small sizes get slightly thicker strokes so the mark
    # survives at 16 px.
    small = size <= 24
    pad = s * 0.055
    tile_r = s * 0.225
    cap_w = s * (0.62 if small else 0.66)
    cap_h = s * (0.30 if small else 0.262)
    cap_cy = s * 0.50
    cap_r = cap_h * 0.5
    dot_r = s * (0.055 if small else 0.048)

    for y in range(size):
        for x in range(size):
            px, py = x + 0.5, y + 0.5
            tile = cov(px, py, s / 2, s / 2, s / 2 - pad, s / 2 - pad, tile_r)
            if tile <= 0.0:
                continue
            # Vertical gradient, slightly brighter at the top like the other
            # DCX app icons.
            t = (py - pad) / max(1.0, s - 2 * pad)
            base = mix(INDIGO_TOP, INDIGO_BOTTOM, smoothstep(0.0, 1.0, t))

            a = 0.0
            rgb = base

            # The island: a white capsule with a soft inner shadow at the bottom.
            cap = cov(px, py, s / 2, cap_cy, cap_w / 2, cap_h / 2, cap_r)
            if cap > 0.0:
                shade = 1.0 - 0.16 * smoothstep(cap_cy, cap_cy + cap_h / 2, py)
                rgb = mix(rgb, tuple(c * shade for c in WHITE), cap)
                a = max(a, cap)

            # Left dot (time) and right activity bar, in the accent colour.
            dx = s / 2 - cap_w * 0.30
            left = cov(px, py, dx, cap_cy, dot_r, dot_r, dot_r)
            if left > 0.0:
                rgb = mix(rgb, ACCENT, left)
                a = max(a, left)
            bar_w = cap_w * 0.26
            bar_h = cap_h * 0.20
            bar = cov(px, py, s / 2 + cap_w * 0.12, cap_cy, bar_w / 2, bar_h / 2, bar_h / 2)
            if bar > 0.0:
                rgb = mix(rgb, ACCENT, bar * 0.92)
                a = max(a, bar)

            i = (y * size + x) * 4
            buf[i + 0] = int(max(0, min(255, round(rgb[0]))))
            buf[i + 1] = int(max(0, min(255, round(rgb[1]))))
            buf[i + 2] = int(max(0, min(255, round(rgb[2]))))
            buf[i + 3] = int(max(0, min(255, round(tile * 255))))
    return buf


def png_bytes(rgba: bytearray, size: int) -> bytes:
    raw = bytearray()
    stride = size * 4
    for y in range(size):
        raw.append(0)  # filter: none
        raw.extend(rgba[y * stride:(y + 1) * stride])

    def chunk(tag: bytes, payload: bytes) -> bytes:
        return (
            struct.pack(">I", len(payload))
            + tag
            + payload
            + struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", ihdr)
        + chunk(b"IDAT", zlib.compress(bytes(raw), 9))
        + chunk(b"IEND", b"")
    )


def write_ico(path: Path, images: dict[int, bytes]) -> None:
    sizes = sorted(images)
    header = struct.pack("<HHH", 0, 1, len(sizes))
    offset = 6 + 16 * len(sizes)
    entries = b""
    data = b""
    for size in sizes:
        blob = images[size]
        entries += struct.pack(
            "<BBBBHHII",
            0 if size >= 256 else size,
            0 if size >= 256 else size,
            0,
            0,
            1,
            32,
            len(blob),
            offset + len(data),
        )
        data += blob
    path.write_bytes(header + entries + data)


def main() -> None:
    OUT_RGBA.parent.mkdir(parents=True, exist_ok=True)
    base = render(64)
    OUT_RGBA.write_bytes(bytes(base))
    images = {size: png_bytes(render(size), size) for size in SIZES}
    write_ico(OUT_ICO, images)
    print(f"wrote {OUT_RGBA.relative_to(ROOT)} ({len(base)} bytes)")
    print(f"wrote {OUT_ICO.relative_to(ROOT)} ({OUT_ICO.stat().st_size} bytes)")


if __name__ == "__main__":
    main()

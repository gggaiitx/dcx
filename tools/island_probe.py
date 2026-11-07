"""Frame-accurate screenshot helper for the information island.

Reads the island's own window rectangle through `FindWindowW`, grabs that region
plus a margin from the screen, and prints the pixel values along a horizontal and
a vertical line crossing it. Used to tell a clipped shadow from a painted
background: a shape that stops at the window edge is geometry, one that keeps
going is DWM composition.

Usage:
    python tools/island_probe.py [--margin 24] [--out out/island_probe.png]
"""

import argparse
import ctypes
import ctypes.wintypes as wt

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
try:
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass

TITLE = "桌面信息岛"


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_long),
        ("top", ctypes.c_long),
        ("right", ctypes.c_long),
        ("bottom", ctypes.c_long),
    ]


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--margin", type=int, default=24)
    ap.add_argument("--out", default="out/island_probe.png")
    args = ap.parse_args()

    hwnd = user32.FindWindowW(None, TITLE)
    if not hwnd:
        print("island window not found")
        return
    r = RECT()
    user32.GetWindowRect(wt.HWND(hwnd), ctypes.byref(r))
    print(f"hwnd={hwnd} rect=({r.left},{r.top},{r.right},{r.bottom}) "
          f"size={r.right - r.left}x{r.bottom - r.top}")

    m = args.margin
    box = (r.left - m, r.top - m, r.right + m, r.bottom + m)
    img = ImageGrab.grab(bbox=box, all_screens=True).convert("RGB")
    img.save(args.out)
    print(f"saved {args.out} {img.size}")

    # Horizontal scan one row below the window top, vertical scan through the
    # middle. Runs of identical colour are collapsed so the output stays short.
    def runs(pixels):
        out = []
        start = 0
        for i in range(1, len(pixels) + 1):
            if i == len(pixels) or pixels[i] != pixels[start]:
                out.append((start, i - 1, pixels[start]))
                start = i
        return out

    mid_y = m + (r.bottom - r.top) // 2
    mid_x = m + (r.right - r.left) // 2
    for name, pts in (
        ("row", [(x, mid_y) for x in range(img.width)]),
        ("col", [(mid_x, y) for y in range(img.height)]),
    ):
        px = [img.getpixel(p) for p in pts]
        print(f"-- {name} runs (index, colour):")
        for a, b, c in runs(px):
            print(f"   {a:4d}-{b:4d} {c}")


if __name__ == "__main__":
    main()

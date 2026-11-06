"""End-to-end verification of the file-box drag-out path.

One script, one run, while the host is alive:

1. drop a test file on the desktop and let the open panel absorb it;
2. drag the first row of the "文档" tab out to a clear spot on the desktop;
3. while the mouse is moving, prove the ghost viewport exists and follows
   (a ~395x55 topless window whose centre tracks the cursor);
4. after release, prove the desktop icon landed under the release point
   (pixel diff against a baseline shot of the same area) and the file's
   hidden attribute is gone.

Usage:
    python tools/verify_drag.py
"""

import ctypes
import ctypes.wintypes as wt
import os
import sys
import time

from PIL import ImageGrab

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from box_input import move, down, up  # noqa: E402

u = ctypes.WinDLL("user32", use_last_error=True)
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass

DESKTOP = r"d:\Users\Administrator\Desktop"
TEST = os.path.join(DESKTOP, "dcx-drag-test.txt")
DPI = 1.25
TILE_X, TILE_Y = 1167, 1050
OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "out")

WNDENUMPROC = ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)


def find_windows(pred):
    res = []

    def cb(h, _):
        if pred(h):
            res.append(h)
        return True

    u.EnumWindows(WNDENUMPROC(cb), 0)
    return res


def rect_of(h):
    r = wt.RECT()
    u.GetWindowRect(h, ctypes.byref(r))
    return (r.left, r.top, r.right, r.bottom)


def title_of(h):
    n = ctypes.create_unicode_buffer(64)
    u.GetWindowTextW(h, n, 64)
    return n.value


def find_panel():
    for h in find_windows(lambda h: "收纳盒" in title_of(h)):
        return h, rect_of(h)
    return None


def find_ghost(cx, cy, tol=90):
    """The ghost viewport: ~GHOST_W*DPI wide, ~GHOST_H*DPI tall, centre near the cursor."""

    def is_ghost(h):
        if not u.IsWindowVisible(h):
            return False
        l, t, r, b = rect_of(h)
        w, hh = r - l, b - t
        if not (380 <= w <= 410 and 48 <= hh <= 64):
            return False
        gx, gy = (l + r) // 2, (t + b) // 2
        return abs(gx - cx) <= tol and abs(gy - cy) <= tol

    hits = find_windows(is_ghost)
    return [(h, rect_of(h)) for h in hits]


def shot(box=None):
    return ImageGrab.grab(bbox=box, all_screens=True)


def region_noise(img, cx, cy, rad=90):
    """How unlike a flat background one screen region is."""
    px = img.convert("L")
    box = (max(cx - rad, 0), max(cy - rad, 0), min(cx + rad, img.width), min(cy + rad, img.height))
    region = px.crop(box)
    hist = region.histogram()
    n = sum(hist)
    mean = sum(i * c for i, c in enumerate(hist)) / max(n, 1)
    var = sum(c * (i - mean) ** 2 for i, c in enumerate(hist)) / max(n, 1)
    return var


def diff_count(a, b, cx, cy, rad=90):
    """Pixels that changed between two shots inside one region."""
    box = (max(cx - rad, 0), max(cy - rad, 0), min(cx + rad, a.width), min(cy + rad, a.height))
    pa, pb = a.convert("L").crop(box), b.convert("L").crop(box)
    da, db = pa.tobytes(), pb.tobytes()
    return sum(1 for x, y in zip(da, db) if abs(x - y) > 24)


def hidden(path):
    try:
        return bool(os.stat(path).st_file_attributes & 0x2)
    except OSError:
        return None


def main():
    os.makedirs(OUT, exist_ok=True)

    # 1. Test file on the desktop; the open panel's refresh absorbs it.
    with open(TEST, "w", encoding="utf-8") as f:
        f.write("drag-out verification\n")
    print(f"test file: {TEST} hidden={hidden(TEST)}")

    # 2. Hover the tile until the panel opens.
    move(TILE_X, TILE_Y)
    time.sleep(1.8)
    panel = find_panel()
    if not panel:
        print("FAIL: panel window not found (host alive? tile present?)")
        return 1
    ph, (pl, pt_, pr, pb) = panel
    pw = pr - pl
    if pw < 600:  # ~560 px max when fully open (348 pt * 1.25 + shadow)
        print(f"panel looks closed (w={pw}); hovering again")
        time.sleep(1.0)
        panel = find_panel()
        ph, (pl, pt_, pr, pb) = panel
        pw = pr - pl
    print(f"panel hwnd={ph} rect={(pl, pt_, pr, pb)} w={pw}")

    # First row centre: shadow + header + tabs, then half a row.
    row_x = (pl + pr) // 2
    row_y = pt_ + int((12 + 44 + 40 + 2 + 15) * DPI)
    print(f"first row centre ≈ ({row_x}, {row_y})")

    # Does the list actually have a row there? Dark pixels inside the row band.
    base = shot()
    band = base.convert("L").crop((pl + 30, row_y - 15, pr - 30, row_y + 15))
    dark = sum(1 for p in band.getdata() if p < 0x80)
    print(f"row band dark pixels: {dark}")
    if dark < 60:
        print("FAIL: no visible row in the 文档 tab — nothing to drag")
        return 1

    # Pick the emptiest release point among candidates.
    cands = [(500, 300), (700, 260), (420, 520), (760, 520), (980, 300)]
    tgt = min(cands, key=lambda c: region_noise(base, *c))
    print(f"release target: {tgt}")

    # 3. The drag itself, with ghost probes on the way.
    move(row_x, row_y)
    time.sleep(0.3)
    down()
    time.sleep(0.15)
    steps = 24
    mx, my = row_x, row_y
    ghost_seen = []
    for i in range(1, steps + 1):
        t = i / steps
        mx = int(row_x + (tgt[0] - row_x) * t)
        my = int(row_y + (tgt[1] - row_y) * t)
        move(mx, my)
        time.sleep(0.35 / steps)
        if i in (steps // 2, steps):
            g = find_ghost(mx, my)
            ghost_seen.append((mx, my, g))
            print(f"  at step {i:2d} mouse=({mx},{my}) ghost={g}")
    mid = shot(box=(tgt[0] - 260, tgt[1] - 160, tgt[0] + 260, tgt[1] + 160))
    mid.save(os.path.join(OUT, "drag_mid.png"))

    time.sleep(0.2)
    up()
    print("released")

    # 4. The icon should appear under the release point.
    time.sleep(1.4)
    after = shot()
    after.save(os.path.join(OUT, "drag_after.png"))
    base.save(os.path.join(OUT, "drag_base.png"))
    changed = diff_count(base, after, *tgt)
    print(f"pixels changed near {tgt}: {changed}")
    gh = find_ghost(*tgt)
    print(f"ghost after release: {gh}")

    ok_ghost = any(g for _, _, g in ghost_seen)
    ok_follow = (
        len(ghost_seen) == 2
        and all(g for _, _, g in ghost_seen)
        and all(abs((g[0][1][0] + g[0][1][2]) // 2 - m) <= 12 for m, _, g in ghost_seen for g in g)
    )
    ok_land = changed > 400 and not gh
    ok_attr = hidden(TEST) is False
    print(f"ghost present while dragging : {'PASS' if ok_ghost else 'FAIL'}")
    print(f"ghost follows the cursor    : {'PASS' if ok_follow else 'FAIL'}")
    print(f"icon landed at release point: {'PASS' if ok_land else 'FAIL'}")
    print(f"hidden attribute cleared    : {'PASS' if ok_attr else 'FAIL'}")
    return 0 if (ok_ghost and ok_land) else 2


if __name__ == "__main__":
    sys.exit(main())

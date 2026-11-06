"""Capture the DCX 收纳盒 panel for visual verification.

Auto-hide is switched off for the duration (an auto-hidden bar slides away and
takes the tile with it), then restored. The cursor is parked where the caller
asks so hover-to-open can be captured deterministically.

Plain ctypes + Pillow — no Add-Type, so it runs in a locked-down shell.

Usage:
    python tools/box_shot.py --out out/box.png [--hover] [--wait 900]
        [--region x,y,w,h]
"""

import argparse
import ctypes
import ctypes.wintypes as wt
import time

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
shell32 = ctypes.WinDLL("shell32", use_last_error=True)
try:
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass

ABM_GETSTATE, ABM_SETSTATE = 0x0004, 0x000A
ABS_AUTOHIDE = 0x0001


class APPBARDATA(ctypes.Structure):
    _fields_ = [
        ("cbSize", wt.DWORD),
        ("hWnd", wt.HWND),
        ("uCallbackMessage", wt.UINT),
        ("uEdge", wt.UINT),
        ("rc", wt.RECT),
        ("lParam", wt.LPARAM),
    ]


def appbar_state():
    d = APPBARDATA()
    d.cbSize = ctypes.sizeof(d)
    return shell32.SHAppBarMessage(ABM_GETSTATE, ctypes.byref(d))


def set_autohide(on: bool):
    d = APPBARDATA()
    d.cbSize = ctypes.sizeof(d)
    d.lParam = ABS_AUTOHIDE if on else 0
    shell32.SHAppBarMessage(ABM_SETSTATE, ctypes.byref(d))


def taskbar_rect():
    h = user32.FindWindowW("Shell_TrayWnd", None)
    r = wt.RECT()
    user32.GetWindowRect(h, ctypes.byref(r))
    return r


def tile_center():
    """Same anchor the panel uses: the slot right after the app-icon area."""
    bar = user32.FindWindowW("Shell_TrayWnd", None)
    r = wt.RECT()
    user32.GetWindowRect(bar, ctypes.byref(r))
    x = None
    rebar = user32.FindWindowExW(bar, 0, "ReBarWindow32", None)
    if rebar:
        rr = wt.RECT()
        user32.GetWindowRect(rebar, ctypes.byref(rr))
        x = rr.right + 6 + 20
    if x is None:
        n = user32.FindWindowExW(bar, 0, "TrayNotifyWnd", None)
        if n:
            nr = wt.RECT()
            user32.GetWindowRect(n, ctypes.byref(nr))
            x = nr.left - 10 - 20
        else:
            x = r.right - 60
    return x, (r.top + r.bottom) // 2


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--out", default="out/box.png")
    ap.add_argument("--hover", action="store_true", help="park the cursor on the tile")
    ap.add_argument("--at", default="", help="explicit cursor position x,y")
    ap.add_argument("--wait", type=int, default=800)
    ap.add_argument("--region", default="", help="x,y,w,h (screen px); default full screen")
    args = ap.parse_args()

    was_autohide = bool(appbar_state() & ABS_AUTOHIDE)
    if was_autohide:
        set_autohide(False)
        time.sleep(0.6)

    try:
        if args.hover or args.at:
            if args.at:
                x, y = (int(v) for v in args.at.split(","))
            else:
                x, y = tile_center()
            user32.SetCursorPos(x, y)
            print(f"cursor -> {x},{y}")
        time.sleep(args.wait / 1000.0)

        if args.region:
            x, y, w, h = (int(v) for v in args.region.split(","))
            box = (x, y, x + w, y + h)
        else:
            box = (0, 0, user32.GetSystemMetrics(0), user32.GetSystemMetrics(1))
        img = ImageGrab.grab(bbox=box, all_screens=True)
        img.save(args.out)
        print(f"saved {args.out} {img.size}")
    finally:
        if was_autohide:
            set_autohide(True)


if __name__ == "__main__":
    main()

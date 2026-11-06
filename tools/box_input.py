"""Drive the file-box panel with real mouse input (SendInput).

Used to verify hover/click/drag behaviour end to end — egui reacts to winit's
raw input, so synthetic input has to go through the OS rather than the app.

Usage:
    python tools/box_input.py click 300 540
    python tools/box_input.py hover 425 1050
    python tools/box_input.py drag 300 600 380 560 --steps 24 --hold 0.4
"""

import argparse
import ctypes
import ctypes.wintypes as wt
import time

u = ctypes.WinDLL("user32", use_last_error=True)
try:
    u.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))
except Exception:
    pass

MOUSEEVENTF_MOVE = 0x0001
MOUSEEVENTF_ABSOLUTE = 0x8000
MOUSEEVENTF_LEFTDOWN = 0x0002
MOUSEEVENTF_LEFTUP = 0x0004
MOUSEEVENTF_VIRTUALDESK = 0x4000
INPUT_MOUSE = 0


class MOUSEINPUT(ctypes.Structure):
    _fields_ = [
        ("dx", ctypes.c_long),
        ("dy", ctypes.c_long),
        ("mouseData", ctypes.c_ulong),
        ("dwFlags", ctypes.c_ulong),
        ("time", ctypes.c_ulong),
        ("dwExtraInfo", ctypes.POINTER(ctypes.c_ulong)),
    ]


class INPUT(ctypes.Structure):
    class _U(ctypes.Union):
        _fields_ = [("mi", MOUSEINPUT)]

    _anonymous_ = ("u",)
    _fields_ = [("type", ctypes.c_ulong), ("u", _U)]


def _send(flags, dx=0, dy=0):
    inp = INPUT(type=INPUT_MOUSE)
    inp.mi = MOUSEINPUT(dx, dy, 0, flags, 0, None)
    u.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))


def move(x, y):
    # SM_CXVIRTUALSCREEN/SM_CYVIRTUALSCREEN are 78/79 (76/77 are the origins),
    # and the origin matters too: a multi-monitor desktop starts left of 0.
    vx = u.GetSystemMetrics(78)
    vy = u.GetSystemMetrics(79)
    ox = u.GetSystemMetrics(76)
    oy = u.GetSystemMetrics(77)
    x -= ox
    y -= oy
    ax = int(x * 65535 / max(vx - 1, 1))
    ay = int(y * 65535 / max(vy - 1, 1))
    _send(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, ax, ay)


def wheel(ticks, x, y):
    move(x, y)
    time.sleep(0.1)
    delta = int(ticks * 120)
    inp = INPUT(type=INPUT_MOUSE)
    inp.mi = MOUSEINPUT(0, 0, delta & 0xFFFFFFFF, 0x0800, 0, None)
    u.SendInput(1, ctypes.byref(inp), ctypes.sizeof(INPUT))


def down():
    _send(MOUSEEVENTF_LEFTDOWN)


def up():
    _send(MOUSEEVENTF_LEFTUP)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("action", choices=["click", "hover", "drag", "move", "wheel"])
    ap.add_argument("coords", nargs="+", type=int)
    ap.add_argument("--steps", type=int, default=20)
    ap.add_argument("--hold", type=float, default=0.35)
    ap.add_argument("--settle", type=float, default=0.6)
    args = ap.parse_args()

    if args.action == "wheel":
        x, y, ticks = args.coords[0], args.coords[1], args.coords[2]
        wheel(ticks, x, y)
        print(f"wheel {ticks} at {x},{y}")
    elif args.action in ("hover", "move", "click"):
        x, y = args.coords[0], args.coords[1]
        move(x, y)
        time.sleep(0.12)
        if args.action == "click":
            down()
            time.sleep(0.06)
            up()
        print(f"{args.action} at {x},{y}")
    else:
        x1, y1, x2, y2 = args.coords[:4]
        move(x1, y1)
        time.sleep(0.25)
        down()
        time.sleep(0.12)
        for i in range(1, args.steps + 1):
            t = i / args.steps
            move(int(x1 + (x2 - x1) * t), int(y1 + (y2 - y1) * t))
            time.sleep(args.hold / args.steps)
        time.sleep(0.25)
        up()
        print(f"drag {x1},{y1} -> {x2},{y2}")
    time.sleep(args.settle)


if __name__ == "__main__":
    main()

"""Capture the taskbar with the dock-hover effect at rest and under the cursor.

Turns auto-hide off for the duration (an auto-hidden bar slides away between
frames and poisons every measurement), restores it afterwards.

No Add-Type / pyautogui: plain ctypes + Pillow, so it runs in a locked-down shell.
"""

import ctypes
import sys
import time

from PIL import ImageGrab

user32 = ctypes.WinDLL("user32", use_last_error=True)
shell32 = ctypes.WinDLL("shell32", use_last_error=True)
try:
    user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))  # PER_MONITOR_AWARE_V2
except Exception:
    pass


class RECT(ctypes.Structure):
    _fields_ = [
        ("left", ctypes.c_int),
        ("top", ctypes.c_int),
        ("right", ctypes.c_int),
        ("bottom", ctypes.c_int),
    ]


class POINT(ctypes.Structure):
    _fields_ = [("x", ctypes.c_long), ("y", ctypes.c_long)]


class APPBARDATA(ctypes.Structure):
    _fields_ = [
        ("cbSize", ctypes.c_ulong),
        ("hWnd", ctypes.c_void_p),
        ("uCallbackMessage", ctypes.c_uint),
        ("uEdge", ctypes.c_uint),
        ("rc", RECT),
        ("lParam", ctypes.c_ssize_t),
    ]


ABM_GETSTATE = 0x00000004
ABM_SETSTATE = 0x0000000A
ABS_AUTOHIDE = 0x00000001


def tray_hwnd():
    h = user32.FindWindowW("Shell_TrayWnd", None)
    if not h:
        raise SystemExit("no Shell_TrayWnd")
    return h


def tray_rect():
    r = RECT()
    if not user32.GetWindowRect(tray_hwnd(), ctypes.byref(r)):
        raise SystemExit("GetWindowRect failed")
    return r


def screen_size():
    return user32.GetSystemMetrics(0), user32.GetSystemMetrics(1)


def get_autohide():
    d = APPBARDATA()
    d.cbSize = ctypes.sizeof(APPBARDATA)
    d.hWnd = tray_hwnd()
    return shell32.SHAppBarMessage(ABM_GETSTATE, ctypes.byref(d))


def set_autohide(on):
    d = APPBARDATA()
    d.cbSize = ctypes.sizeof(APPBARDATA)
    d.hWnd = tray_hwnd()
    state = shell32.SHAppBarMessage(ABM_GETSTATE, ctypes.byref(d))
    state = (state | ABS_AUTOHIDE) if on else (state & ~ABS_AUTOHIDE)
    d.lParam = state
    shell32.SHAppBarMessage(ABM_SETSTATE, ctypes.byref(d))


def cursor():
    p = POINT()
    user32.GetCursorPos(ctypes.byref(p))
    return p.x, p.y


def move(x, y):
    user32.SetCursorPos(int(x), int(y))


def shot(path, rect, pad_above=110):
    box = (rect.left, max(0, rect.top - pad_above), rect.right, rect.bottom)
    ImageGrab.grab(bbox=box, all_screens=True).save(path)
    print(f"{path}  {box}")


def main():
    out = sys.argv[1] if len(sys.argv) > 1 else "out"
    before = cursor()
    was_hidden = bool(get_autohide() & ABS_AUTOHIDE)
    if was_hidden:
        set_autohide(False)
        time.sleep(1.2)

    try:
        r = tray_rect()
        print(f"tray {r.left},{r.top} - {r.right},{r.bottom}")

        # Rest: pointer inside the bar but left of the first icon, so the bar is
        # visible and every icon sits at scale 1.
        move(r.left + 6, (r.top + r.bottom) // 2)
        time.sleep(0.6)
        r = tray_rect()
        shot(f"{out}/hover-rest.png", r)

        mid = (r.left + r.right) // 2
        for label, x in (("mid", mid), ("left", mid - 380)):
            move(x, (r.top + r.bottom) // 2)
            time.sleep(0.6)
            shot(f"{out}/hover-{label}.png", r)
    finally:
        move(*before)
        if was_hidden:
            set_autohide(True)
        print("restored")


if __name__ == "__main__":
    main()

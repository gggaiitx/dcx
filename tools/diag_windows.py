"""Enumerate shell / XAML-host top-level windows with geometry, styles and DWM state.

Diagnostic only: reads window metadata, never changes anything.
    python tools/diag_windows.py            # all windows
    python tools/diag_windows.py --shell    # only shell XAML hosts
"""

import ctypes
import ctypes.wintypes as wt
import sys

user32 = ctypes.WinDLL("user32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
try:
    shcore = ctypes.WinDLL("shcore", use_last_error=True)
except OSError:
    shcore = None

GWL_STYLE = -16
GWL_EXSTYLE = -20
DWMWA_CLOAK = 13
DWMWA_EXTENDED_FRAME_BOUNDS = 9
DWMWA_CORNER_PREFERENCE = 33
DWMWA_VISIBLE_FRAME_BORDER_THICKNESS = 37

SHELL_PROCS = {
    "StartMenuExperienceHost.exe",
    "SearchHost.exe",
    "ShellExperienceHost.exe",
    "explorer.exe",
    "TextInputHost.exe",
}

WS_CAPTION = 0x00C00000
WS_THICKFRAME = 0x00040000
WS_POPUP = 0x80000000
WS_BORDER = 0x00800000
WS_EX_LAYERED = 0x00080000
WS_EX_NOREDIRECTIONBITMAP = 0x00200000
WS_EX_TOOLWINDOW = 0x00000080
WS_EX_APPWINDOW = 0x00040000


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


def proc_name(pid):
    PROCESS_QUERY_LIMITED_INFORMATION = 0x1000
    h = kernel32.OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid)
    if not h:
        return ""
    try:
        buf = ctypes.create_unicode_buffer(1024)
        size = wt.DWORD(1024)
        if not kernel32.QueryFullProcessImageNameW(h, 0, buf, ctypes.byref(size)):
            return ""
        return buf.value.rsplit("\\", 1)[-1]
    finally:
        kernel32.CloseHandle(h)


def enum_windows():
    out = []
    CB = ctypes.WINFUNCTYPE(ctypes.c_bool, wt.HWND, wt.LPARAM)

    def cb(hwnd, _):
        out.append(hwnd)
        return True

    user32.EnumWindows(CB(cb), 0)
    return out


def main():
    shell_only = "--shell" in sys.argv
    try:
        dpi = user32.GetDpiForSystem()
    except Exception:
        dpi = 0
    print(f"screen {user32.GetSystemMetrics(0)}x{user32.GetSystemMetrics(1)}  dpi={dpi} "
          f"scale={dpi/96:.2f}  workarea (logical)="
          f"{user32.GetSystemMetrics(0)/dpi*96:.0f}x{user32.GetSystemMetrics(1)/dpi*96:.0f}")
    print()
    rows = []
    for hwnd in enum_windows():
        pid = wt.DWORD()
        user32.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
        name = proc_name(pid.value)
        if shell_only and name not in SHELL_PROCS:
            continue
        cls = ctypes.create_unicode_buffer(256)
        user32.GetClassNameW(hwnd, cls, 256)
        title = ctypes.create_unicode_buffer(256)
        user32.GetWindowTextW(hwnd, title, 256)
        r = RECT()
        user32.GetWindowRect(hwnd, ctypes.byref(r))
        w, h = r.right - r.left, r.bottom - r.top
        if w <= 0 and h <= 0:
            continue
        style = user32.GetWindowLongW(hwnd, GWL_STYLE) & 0xFFFFFFFF
        ex = user32.GetWindowLongW(hwnd, GWL_EXSTYLE) & 0xFFFFFFFF
        cloaked = ctypes.c_int(0)
        dwmapi.DwmGetWindowAttribute(hwnd, DWMWA_CLOAK, ctypes.byref(cloaked), 4)
        corner = ctypes.c_int(-1)
        dwmapi.DwmGetWindowAttribute(hwnd, DWMWA_CORNER_PREFERENCE, ctypes.byref(corner), 4)
        thick = ctypes.c_uint(-1)
        dwmapi.DwmGetWindowAttribute(hwnd, DWMWA_VISIBLE_FRAME_BORDER_THICKNESS,
                                     ctypes.byref(thick), 4)
        vis = "vis" if user32.IsWindowVisible(hwnd) else "hid"
        flags = []
        if style & WS_CAPTION == WS_CAPTION:
            flags.append("CAPTION")
        if style & WS_THICKFRAME:
            flags.append("THICKFRAME")
        if style & WS_POPUP:
            flags.append("POPUP")
        if style & WS_BORDER:
            flags.append("BORDER")
        if ex & WS_EX_LAYERED:
            flags.append("LAYERED")
        if ex & WS_EX_NOREDIRECTIONBITMAP:
            flags.append("NOREDIR")
        if ex & WS_EX_TOOLWINDOW:
            flags.append("TOOL")
        if cloaked.value:
            flags.append("CLOAKED")
        rows.append((name, cls.value, title.value, vis, r.left, r.top, w, h,
                     corner.value, thick.value, ",".join(flags), hwnd))
    rows.sort(key=lambda t: (t[0], -t[6] * t[7]))
    fmt = "{:<26} {:<44} {:<22} {:<4} {:>6} {:>5} {:>5} {:>5}  cp={:<3} bt={:<4} {}"
    for t in rows:
        print(fmt.format(t[0], t[1][:44], t[2][:22], t[3], t[4], t[5], t[6], t[7],
                         t[8], t[9], t[10]))


if __name__ == "__main__":
    main()

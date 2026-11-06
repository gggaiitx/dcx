"""Reset DWM frame attributes on shell surfaces that are already poisoned.

`DWMWA_WINDOW_CORNER_PREFERENCE` is sticky: a window keeps the value until it is
destroyed, so a flyout styled by an older dcx-host keeps its hollow frame even
after the host exits. This walks the shell windows that exist right now and puts
the two frame attributes back:

    corner preference -> DWMWCP_DONOTROUND
    border color      -> DWMWA_COLOR_NONE

Only shell classes / shell host processes are touched, so normal application
windows keep the corners the effect gave them.

    python tools/clear_shell_frames.py            # report + repair
    python tools/clear_shell_frames.py --dry-run  # report only
"""

import ctypes
import ctypes.wintypes as wt
import sys

user32 = ctypes.WinDLL("user32", use_last_error=True)
dwmapi = ctypes.WinDLL("dwmapi", use_last_error=True)
kernel32 = ctypes.WinDLL("kernel32", use_last_error=True)
user32.SetProcessDpiAwarenessContext(ctypes.c_void_p(-4))

DWMWA_WINDOW_CORNER_PREFERENCE = 33
DWMWA_BORDER_COLOR = 34
DWMWA_COLOR_NONE = 0xFFFF_FFFE
DWMWCP_DEFAULT = 0
DWMWCP_DONOTROUND = 1

SHELL_CLASSES = (
    "Windows.UI.Core.CoreWindow",
    "Xaml_WindowedPopupClass",
    "TopLevelWindowForOverflowXamlIsland",
    "Windows.UI.Composition.DesktopWindowContentBridge",
    "Shell_TrayWnd",
    "Shell_SecondaryTrayWnd",
    "Progman",
    "tooltips_class32",
)
SHELL_HOSTS = (
    "startmenuexperiencehost.exe",
    "shellexperiencehost.exe",
    "searchhost.exe",
    "explorer.exe",
)


class RECT(ctypes.Structure):
    _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long),
                ("right", ctypes.c_long), ("bottom", ctypes.c_long)]


def exe_of(h):
    pid = wt.DWORD()
    user32.GetWindowThreadProcessId(h, ctypes.byref(pid))
    ph = kernel32.OpenProcess(0x1000, 0, pid.value)
    if not ph:
        return ""
    try:
        buf = ctypes.create_unicode_buffer(1024)
        n = wt.DWORD(1024)
        if kernel32.QueryFullProcessImageNameW(ph, 0, buf, ctypes.byref(n)):
            return buf.value.rsplit("\\", 1)[-1]
        return ""
    finally:
        kernel32.CloseHandle(ph)


def cls_of(h):
    buf = ctypes.create_unicode_buffer(512)
    user32.GetClassNameW(h, buf, 512)
    return buf.value


def get_corner(h):
    v = ctypes.c_int(-1)
    hr = dwmapi.DwmGetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE,
                                      ctypes.byref(v), 4)
    return v.value if hr == 0 else None


def set_corner(h, v):
    val = ctypes.c_int(v)
    return dwmapi.DwmSetWindowAttribute(h, DWMWA_WINDOW_CORNER_PREFERENCE,
                                        ctypes.byref(val), 4) == 0


def set_border_none(h):
    val = ctypes.c_uint(DWMWA_COLOR_NONE)
    return dwmapi.DwmSetWindowAttribute(h, DWMWA_BORDER_COLOR,
                                        ctypes.byref(val), 4) == 0


def candidates():
    """Z-order walk + a screen-wide hit test grid: some shell islands are not
    returned by EnumWindows on this build, but WindowFromPoint still finds them."""
    seen = []
    h = user32.GetTopWindow(None)
    while h:
        seen.append(h)
        h = user32.GetWindow(h, 2)  # GW_HWNDNEXT
    for y in range(20, 1080, 60):
        for x in range(20, 1920, 60):
            h = user32.WindowFromPoint(wt.POINT(x, y))
            root = user32.GetAncestor(h, 2)  # GA_ROOT
            if root:
                seen.append(root)
    seen.append(user32.GetForegroundWindow())
    out, dedup = [], set()
    for h in seen:
        if h and h not in dedup:
            dedup.add(h)
            out.append(h)
    return out


def main():
    dry = "--dry-run" in sys.argv
    rows = []
    for h in candidates():
        exe = exe_of(h)
        cls = cls_of(h)
        if not (exe.lower() in SHELL_HOSTS or cls in SHELL_CLASSES):
            continue
        cp = get_corner(h)
        if cp in (None, DWMWCP_DEFAULT, DWMWCP_DONOTROUND):
            continue
        r = RECT()
        user32.GetWindowRect(h, ctypes.byref(r))
        rows.append((h, exe, cls, cp, r))

    if not rows:
        print("no shell window carries a non-default corner preference")
        return

    print(f"{'process':<26}{'class':<38}{'size':<12}{'cp':>4}  action")
    for h, exe, cls, cp, r in rows:
        size = f"{r.right - r.left}x{r.bottom - r.top}"
        if dry:
            action = "would reset"
        else:
            ok = set_corner(h, DWMWCP_DONOTROUND)
            set_border_none(h)
            action = "reset" if ok else "reset failed"
        print(f"{exe:<26}{cls[:38]:<38}{size:<12}{cp:>4}  {action}")


if __name__ == "__main__":
    main()

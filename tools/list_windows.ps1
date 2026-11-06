# List every top-level window (visible or not) with its owning process.
# Usage: powershell -File tools/list_windows.ps1
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public struct L_RECT { public int Left, Top, Right, Bottom; }
public static class LW {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetClassNameW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out L_RECT r);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
    public static List<string> Dump() {
        var list = new List<string>();
        EnumWindows((h, p) => {
            var t = new StringBuilder(512); GetWindowTextW(h, t, 512);
            var c = new StringBuilder(512); GetClassNameW(h, c, 512);
            uint pid; GetWindowThreadProcessId(h, out pid);
            L_RECT r; GetWindowRect(h, out r);
            if (t.Length > 0 || c.ToString().Contains("egui") || c.ToString().Contains("Window"))
                list.Add(string.Format("{0} title='{1}' class='{2}' vis={3} pid={4} rect={5},{6},{7},{8}",
                    h, t, c, IsWindowVisible(h), pid, r.Left, r.Top, r.Right, r.Bottom));
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
"@
[LW]::Dump() | Where-Object { $_ -match "DCX|egui|pid=25720|vis=True" }

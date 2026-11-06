# Capture a top-level window by title substring into a PNG.
#
# PrintWindow(PW_RENDERFULLCONTENT) is used instead of a screen grab so the
# capture is not affected by other windows covering it.
#
# Usage: pwsh -File tools/win_shot.ps1 -Title "DCX 设置" -Out out/settings.png
param(
    [string]$Title = "DCX",
    [string]$Out = "out\window.png",
    [int]$WaitMs = 6000,
    [string]$LaunchExe = ""
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public struct W_RECT { public int Left, Top, Right, Bottom; }
public static class WinShot {
    public delegate bool EnumProc(IntPtr h, IntPtr p);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumProc cb, IntPtr p);
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern int GetWindowTextW(IntPtr h, StringBuilder s, int n);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr h);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out W_RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
    public static List<IntPtr> Find(string needle) {
        var list = new List<IntPtr>();
        EnumWindows((h, p) => {
            if (!IsWindowVisible(h)) return true;
            var sb = new StringBuilder(512);
            GetWindowTextW(h, sb, 512);
            var t = sb.ToString();
            if (t.Length > 0 && t.Contains(needle)) list.Add(h);
            return true;
        }, IntPtr.Zero);
        return list;
    }
}
"@

[WinShot]::SetProcessDPIAware() | Out-Null

$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$outPath = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $root $Out }
$dir = Split-Path -Parent $outPath
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

if ($LaunchExe -ne "") {
    Start-Process -FilePath $LaunchExe -ArgumentList "--settings" | Out-Null
    Start-Sleep -Milliseconds 2200
}

$h = [IntPtr]::Zero
$deadline = (Get-Date).AddMilliseconds($WaitMs)
while ((Get-Date) -lt $deadline) {
    $found = [WinShot]::Find($Title)
    if ($found.Count -gt 0) { $h = $found[0]; break }
    Start-Sleep -Milliseconds 200
}
if ($h -eq [IntPtr]::Zero) { Write-Error "window containing '$Title' not found"; exit 1 }

[WinShot]::SetForegroundWindow($h) | Out-Null
Start-Sleep -Milliseconds 400

$r = New-Object W_RECT
[WinShot]::GetWindowRect($h, [ref]$r) | Out-Null
$w = $r.Right - $r.Left
$hh = $r.Bottom - $r.Top
if ($w -le 0 -or $hh -le 0) { Write-Error "bad window rect"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap($w, $hh)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
$ok = [WinShot]::PrintWindow($h, $hdc, 2)
$g.ReleaseHdc($hdc)
$g.Dispose()
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $outPath ($w x $hh) printwindow=$ok origin=$($r.Left),$($r.Top)"

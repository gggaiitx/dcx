# Capture the taskbar into a PNG for visual verification.
#
# The taskbar is a DirectComposition surface: a plain BitBlt of the screen does
# not always contain it (observed on this machine), so capture the taskbar
# window itself with PrintWindow(PW_RENDERFULLCONTENT) instead.
#
# Usage: pwsh -File tools/shot.ps1 [-Out out\taskbar.png]
param(
    [string]$Out = "out\taskbar.png"
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public struct RECT { public int Left, Top, Right, Bottom; }
public static class Shot {
    [DllImport("user32.dll", CharSet = CharSet.Unicode)] public static extern IntPtr FindWindow(string cls, string name);
    [DllImport("user32.dll")] public static extern bool GetWindowRect(IntPtr h, out RECT r);
    [DllImport("user32.dll")] public static extern bool PrintWindow(IntPtr h, IntPtr hdc, uint flags);
    [DllImport("user32.dll")] public static extern bool SetProcessDPIAware();
}
"@

# Without this the process receives DPI-virtualized coordinates (for example
# 1536x48 for a 1920x60 taskbar at 125%), so PrintWindow renders only the left
# part of the XAML surface and the tray clock falls outside the bitmap.
[Shot]::SetProcessDPIAware() | Out-Null

$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$outPath = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $root $Out }
$dir = Split-Path -Parent $outPath
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

$tray = [Shot]::FindWindow("Shell_TrayWnd", $null)
if ($tray -eq [IntPtr]::Zero) { Write-Error "Shell_TrayWnd not found"; exit 1 }

$r = New-Object RECT
[Shot]::GetWindowRect($tray, [ref]$r) | Out-Null
$w = $r.Right - $r.Left
$h = $r.Bottom - $r.Top
if ($w -le 0 -or $h -le 0) { Write-Error "bad taskbar rect"; exit 1 }

$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$hdc = $g.GetHdc()
# PW_RENDERFULLCONTENT = 2: needed for DirectComposition / XAML content.
$ok = [Shot]::PrintWindow($tray, $hdc, 2)
$g.ReleaseHdc($hdc)
$g.Dispose()
$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $outPath ($w x $h) printwindow=$ok"

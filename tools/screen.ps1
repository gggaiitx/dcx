# Capture the whole screen from the DWM-composited output.
#
# Unlike tools/shot.ps1 (PrintWindow) this never sends a message to the taskbar
# window, so it still works when the taskbar UI thread is hung.
#
# Usage: pwsh -File tools/screen.ps1 [-Out out\screen.png] [-Crop x,y,w,h]
param(
    [string]$Out = "out\screen.png",
    [string]$Crop = ""
)

Add-Type -AssemblyName System.Drawing

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Scr {
    [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
}
"@

$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$outPath = if ([System.IO.Path]::IsPathRooted($Out)) { $Out } else { Join-Path $root $Out }
$dir = Split-Path -Parent $outPath
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }

$w = [Scr]::GetSystemMetrics(0)
$h = [Scr]::GetSystemMetrics(1)
$dc = [Scr]::GetDC([IntPtr]::Zero)
$bmp = New-Object System.Drawing.Bitmap($w, $h)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0, 0, 0, 0, (New-Object System.Drawing.Size($w, $h)))
$g.Dispose()
[Scr]::ReleaseDC([IntPtr]::Zero, $dc) | Out-Null

if ($Crop -ne "") {
    $p = $Crop.Split(",") | ForEach-Object { [int]$_ }
    $rect = New-Object System.Drawing.Rectangle($p[0], $p[1], $p[2], $p[3])
    $sub = $bmp.Clone($rect, $bmp.PixelFormat)
    $bmp.Dispose()
    $bmp = $sub
}

$bmp.Save($outPath, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "saved $outPath (screen $w x $h)"

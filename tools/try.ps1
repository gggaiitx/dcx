# One-shot live test cycle: restart explorer, build, inject, then measure.
#
# The verdict that matters after an injection is whether the taskbar UI thread
# is healthy, so this reports CPU burn (a spin shows up as >100%), window
# responsiveness, the non-VERIFY log lines and a screen strip — in one run.
#
# Usage:
#   pwsh -File tools/try.ps1               # restart + build + inject + measure
#   pwsh -File tools/try.ps1 -NoBuild      # skip cargo
#   pwsh -File tools/try.ps1 -NoRestart    # inject into the running explorer
param(
    [switch]$NoBuild,
    [switch]$NoRestart,
    [switch]$NoInject,
    [int]$WaitSec = 16,
    [string]$Injector = "D:\cargo-dbx-target\release\wh_injector.exe",
    [string]$Dll = "D:\cargo-dbx-target\release\dcx_engine.dll",
    [string]$Tag = "run"
)

$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$log = Join-Path $env:TEMP 'dcx-engine.log'

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Win {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@

function Get-ShellPid {
    for ($i = 0; $i -lt 40; $i++) {
        $h = [Win]::FindWindowW("Shell_TrayWnd", $null)
        if ($h -ne [IntPtr]::Zero) {
            $p = 0
            [Win]::GetWindowThreadProcessId($h, [ref]$p) | Out-Null
            if ($p -ne 0 -and (Get-Process -Id $p -ErrorAction SilentlyContinue)) { return [int]$p }
        }
        Start-Sleep -Milliseconds 500
    }
    return 0
}

if (-not $NoRestart) {
    Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
}

if (-not $NoBuild) {
    Push-Location $root
    $out = (cargo build --release -p dcx-engine 2>&1 | Out-String)
    $code = $LASTEXITCODE
    Pop-Location
    if ($code -ne 0) {
        Write-Host "BUILD FAILED"
        Write-Host ($out -split "`n" | Select-Object -Last 25)
        exit 1
    }
}

if (-not (Test-Path $Dll)) { Write-Error "missing $Dll"; exit 1 }
Remove-Item $log -ErrorAction SilentlyContinue

if (-not $NoRestart) {
    # winlogon restarts the shell on its own once explorer is killed. Launching
    # `explorer.exe` here as well produces a *second* process that is a file
    # browser, not the shell - injecting into that finds no taskbar XAML at all.
    Start-Sleep -Seconds 4
    if (-not (Get-Process explorer -ErrorAction SilentlyContinue)) {
        Start-Process explorer.exe
        Start-Sleep -Seconds 4
    }
}

# The shell is, by definition, the process that owns the taskbar window.
$shellPid = Get-ShellPid
if ($shellPid -eq 0) { Write-Error "no process owns Shell_TrayWnd"; exit 1 }
Write-Host "shell pid=$shellPid"

if ($NoInject) {
    Write-Host "control run: no injection"
} else {
    & $Injector $shellPid $Dll | Out-Null
    if ($LASTEXITCODE -ne 0) { Write-Error "injection failed"; exit 1 }
}

Start-Sleep -Seconds $WaitSec

# --- verdict ---------------------------------------------------------------
$pid2 = $shellPid
$a = (Get-Process -Id $pid2 -ErrorAction SilentlyContinue).CPU
Start-Sleep -Seconds 3
$b = (Get-Process -Id $pid2 -ErrorAction SilentlyContinue).CPU
$pct = if ($null -ne $a -and $null -ne $b) { [math]::Round((($b - $a) / 3) * 100) } else { 'gone' }
$proc = Get-Process -Id $pid2 -ErrorAction SilentlyContinue
$responding = if ($proc) { $proc.Responding } else { 'no process' }

Write-Host ""
Write-Host "===== VERDICT ($Tag) ====="
Write-Host "cpu      = $pct %   (a healthy idle explorer is < 5%)"
Write-Host "respond  = $responding"

Add-Type -AssemblyName System.Drawing
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class Scr2 {
    [DllImport("user32.dll")] public static extern IntPtr GetDC(IntPtr h);
    [DllImport("user32.dll")] public static extern int ReleaseDC(IntPtr h, IntPtr dc);
    [DllImport("user32.dll")] public static extern int GetSystemMetrics(int n);
}
"@
$sw = [Scr2]::GetSystemMetrics(0); $sh = [Scr2]::GetSystemMetrics(1)
$dc = [Scr2]::GetDC([IntPtr]::Zero)
$bmp = New-Object System.Drawing.Bitmap($sw, $sh)
$g = [System.Drawing.Graphics]::FromImage($bmp)
$g.CopyFromScreen(0, 0, 0, 0, (New-Object System.Drawing.Size($sw, $sh)))
$g.Dispose()
[Scr2]::ReleaseDC([IntPtr]::Zero, $dc) | Out-Null
$shot = Join-Path $root "out\try_$Tag.png"
$dir = Split-Path -Parent $shot
if (-not (Test-Path $dir)) { New-Item -ItemType Directory -Path $dir -Force | Out-Null }
$bmp.Save($shot, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
Write-Host "screen   = $shot"

if (Test-Path $log) {
    Write-Host "----- log (VERIFY lines folded out) -----"
    Get-Content $log | Where-Object { $_ -notmatch 'VERIFY' } | Select-Object -Last 45
} else {
    Write-Host "(no log produced - DLL never ran)"
}

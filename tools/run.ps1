# Build the engine DLL, restart explorer, inject, and dump the log.
#
# The DLL is loaded into explorer while it runs, so the file is locked: the
# restart has to happen *before* cargo tries to write the artifact.
#
# Usage:
#   pwsh -File tools/run.ps1                 # restart explorer, build, inject, tail log
#   pwsh -File tools/run.ps1 -NoRestart      # build + inject into the current explorer
#   pwsh -File tools/run.ps1 -NoBuild        # skip cargo, just restart/inject
#   pwsh -File tools/run.ps1 -WaitSec 8      # how long to wait after injecting
param(
    [switch]$NoBuild,
    [switch]$NoRestart,
    [int]$WaitSec = 8,
    [string]$Injector = "D:\cargo-dbx-target\release\wh_injector.exe",
    [string]$Dll = "D:\cargo-dbx-target\release\dcx_engine.dll"
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $PSCommandPath)
$log = Join-Path $env:TEMP 'dcx-engine.log'

if (-not $NoRestart) {
    Write-Host "restarting explorer..."
    Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
    Start-Sleep -Milliseconds 800
}

if (-not $NoBuild) {
    Push-Location $root
    try {
        $out = cargo build --release -p dcx-engine 2>&1
        $code = $LASTEXITCODE
        $out | Select-Object -Last 25 | ForEach-Object { Write-Host $_ }
        if ($code -ne 0) { Write-Error "cargo build failed ($code)"; exit 1 }
    } finally { Pop-Location }
}

if (-not (Test-Path $Dll)) { Write-Error "missing $Dll"; exit 1 }

Remove-Item $log -ErrorAction SilentlyContinue

if (-not $NoRestart) {
    Start-Process explorer.exe
    Start-Sleep -Seconds 3
}

# The shell is by definition the process that owns the taskbar window. Picking
# "the newest explorer" is wrong: `Start-Process explorer.exe` when a shell is
# already running opens a *file browser* process instead, and injecting into
# that finds no taskbar XAML at all.
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ShellOwner {
    [DllImport("user32.dll", CharSet=CharSet.Unicode)] public static extern IntPtr FindWindowW(string c, string n);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h, out uint pid);
}
"@
$trayHwnd = [ShellOwner]::FindWindowW("Shell_TrayWnd", $null)
$shellPid = 0
if ($trayHwnd -ne [IntPtr]::Zero) { [ShellOwner]::GetWindowThreadProcessId($trayHwnd, [ref]$shellPid) | Out-Null }
if ($shellPid -eq 0) { Write-Error "no process owns Shell_TrayWnd"; exit 1 }
Write-Host "shell pid=$shellPid"

if (-not (Test-Path $Injector)) { Write-Error "missing injector $Injector"; exit 1 }
& $Injector $shellPid $Dll
if ($LASTEXITCODE -ne 0) { Write-Error "injection failed"; exit 1 }

Start-Sleep -Seconds $WaitSec

if (Test-Path $log) {
    Write-Host "===== log ($log) ====="
    Get-Content $log
} else {
    Write-Host "(no log produced - DLL never ran)"
}

# Launch the settings window once and walk through every page, capturing PNGs.
#
# Each step drives real mouse input (python tools/box_input.py) because egui
# reacts to winit's raw input, then prints the window with PrintWindow.
$Root = "C:\Users\Administrator\WorkBuddy\Windhawk\dcx"
$Exe = "D:\cargo-dbx-target\release\dcx-host.exe"
$Python = "C:\Users\Administrator\.workbuddy\binaries\python\versions\3.13.12\python.exe"
$py = Join-Path $Root "tools\box_input.py"
$shot = Join-Path $Root "tools\win_shot.ps1"

$proc = Start-Process -FilePath $Exe -ArgumentList "--settings" -PassThru
Start-Sleep -Milliseconds 2500

function Shoot([string]$name) {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $shot -Title "DCX 设置" -Out "out\$name.png" -WaitMs 3000 | Out-Host
}

function Click([int]$x, [int]$y) {
    & $Python $py click $x $y --settle 0.8 | Out-Host
    Start-Sleep -Milliseconds 300
}

function Wheel([int]$ticks) {
    & $Python $py wheel 700 600 $ticks | Out-Host
    Start-Sleep -Milliseconds 400
}

Shoot "final-look"
Click 470 223
Wheel -16
Shoot "final-rules"
Click 470 277
Shoot "final-about"

Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue

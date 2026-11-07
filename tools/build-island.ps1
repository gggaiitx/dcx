# build-island.ps1 — cargo build with a live progress card on the island.
#
#   .\build-island.ps1                      # debug build of dcx-island
#   .\build-island.ps1 -Profile release -Package dcx-host
#
# The card is keyed by package name, so two builds of different packages do
# not fight over one card. Every "Compiling <crate>" line updates it; the final
# push reports success or failure and lingers a little longer.

param(
    [string]$Package = "dcx-island",
    [ValidateSet("dev", "release")]
    [string]$Profile = "dev",
    [int]$Port = 9841,
    # Rough crate count for the progress bar; only a display hint, the bar
    # still ends at 95 % until cargo actually exits 0.
    [int]$ExpectedCrates = 40,
    [switch]$NoPush
)

$ErrorActionPreference = "Continue"
$root = Split-Path -Parent $PSScriptRoot

function Push-Card {
    param([string]$Title, [string]$Body, [double]$Progress, [int]$Ttl)
    if ($NoPush) { return }
    try {
        & "$PSScriptRoot\island-push.ps1" -Id "build-$Package" -Title $Title -Body $Body `
            -Progress $Progress -Ttl $Ttl -Port $Port | Out-Null
    }
    catch {
        # A missing island must not break the build itself.
        Write-Warning "推送卡片失败：$($_.Exception.Message)"
    }
}

$cardTitle = if ($Profile -eq "release") { "构建 $Package（release）" } else { "构建 $Package" }
Push-Card -Title $cardTitle -Body "准备编译…" -Progress 0 -Ttl 900

$compiled = 0
$tail = New-Object System.Collections.Generic.Queue[string]
$cargoArgs = @("build", "-p", $Package)
if ($Profile -eq "release") { $cargoArgs += "--release" }

& cargo @cargoArgs 2>&1 | ForEach-Object {
    $line = "$_"
    Write-Host $line
    if ($line -match "^\s*Compiling\s+(\S+)") {
        $compiled++
        while ($tail.Count -ge 6) { $tail.Dequeue() | Out-Null }
        $tail.Enqueue($line.Trim())
        $pct = [Math]::Min(0.95, [Math]::Max(0.03, $compiled / [double]$ExpectedCrates))
        Push-Card -Title $cardTitle -Body ("正在编译 {0}（{1}/{2}）" -f $matches[1], $compiled, $ExpectedCrates) `
            -Progress $pct -Ttl 900
    }
    elseif ($line -match "^\s*error(\[|:)") {
        while ($tail.Count -ge 6) { $tail.Dequeue() | Out-Null }
        $tail.Enqueue($line.Trim())
    }
}

if ($LASTEXITCODE -eq 0) {
    $out = if ($Profile -eq "release") { "target\release" } else { "target\debug" }
    Push-Card -Title "$Package 构建成功" -Body ("{0} 个 crate · {1}" -f $compiled, $out) -Progress 1.0 -Ttl 12
    exit 0
}

$detail = ($tail.ToArray() -join "`n")
if (-not $detail) { $detail = "cargo 退出码 $LASTEXITCODE" }
Push-Card -Title "$Package 构建失败" -Body $detail -Progress 1.0 -Ttl 30
exit $LASTEXITCODE

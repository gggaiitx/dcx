# island-push.ps1 — post one card to the island's local push API.
#
#   .\island-push.ps1 -Title "备份完成" -Body "3 个目录 · 12 分钟" -Ttl 30
#   .\island-push.ps1 -Id build -Title "构建中" -Progress 0.4 -Ttl 600
#   .\island-push.ps1 -Remove build
#
# The island binds 127.0.0.1 only; the bearer token is written next to
# island.toml by the island itself (island_api_token.txt). Nothing here reads
# or writes any other DCX state.

param(
    [string]$Id = "",
    [Parameter(Mandatory = $true, ParameterSetName = "push")]
    [string]$Title,
    [string]$Body = "",
    # 0..1; omit for a plain card. Reusing -Id updates the card and renews TTL.
    [double]$Progress = -1,
    [int]$Ttl = 20,
    [ValidateSet("", "open_url", "open_folder", "show_panel")]
    [string]$Action = "",
    [string]$Url = "",
    [string]$Folder = "",
    [string]$Tab = "tasks",
    [Parameter(Mandatory = $true, ParameterSetName = "remove")]
    [string]$Remove,
    [int]$Port = 9841
)

$ErrorActionPreference = "Stop"

$tokenPath = Join-Path $env:LOCALAPPDATA "dcx\island_api_token.txt"
if (-not (Test-Path $tokenPath)) {
    Write-Error "找不到 $tokenPath —— 先启动一次信息岛，它会生成令牌"
    exit 1
}
$token = (Get-Content -Path $tokenPath -Raw).Trim()
if (-not $token) {
    Write-Error "令牌文件为空：$tokenPath"
    exit 1
}

$base = "http://127.0.0.1:$Port"
$headers = @{ Authorization = "Bearer $token" }

try {
    if ($PSCmdlet.ParameterSetName -eq "remove") {
        $reply = Invoke-RestMethod -Method Delete -Uri "$base/v1/island/push/$Remove" `
            -Headers $headers -TimeoutSec 10
    }
    else {
        if (-not $Id) {
            $Id = "push-" + [guid]::NewGuid().ToString("N").Substring(0, 12)
        }
        $card = [ordered]@{ id = $Id; title = $Title; ttl_secs = $Ttl }
        if ($Body) { $card.body = $Body }
        if ($Progress -ge 0) {
            $card.progress = [Math]::Min(1.0, [Math]::Max(0.0, $Progress))
        }
        switch ($Action) {
            "open_url" { $card.action = @{ kind = "open_url"; url = $Url } }
            "open_folder" { $card.action = @{ kind = "open_folder"; path = $Folder } }
            "show_panel" { $card.action = @{ kind = "show_panel"; tab = $Tab } }
        }
        $json = $card | ConvertTo-Json -Compress -Depth 4
        # Send UTF-8 bytes explicitly: Windows PowerShell 5.1 otherwise posts
        # the body as ISO-8859-1 and Chinese titles arrive as '?'.
        $bytes = [Text.Encoding]::UTF8.GetBytes($json)
        $reply = Invoke-RestMethod -Method Post -Uri "$base/v1/island/push" `
            -Headers $headers -ContentType "application/json; charset=utf-8" `
            -Body $bytes -TimeoutSec 10
    }
}
catch {
    Write-Error "推送失败：$($_.Exception.Message)"
    exit 1
}

$reply | ConvertTo-Json -Compress -Depth 4

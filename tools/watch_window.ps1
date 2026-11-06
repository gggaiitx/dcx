# Watch dcx-host.exe for a few seconds and report its main window handle.
param([int]$Seconds = 8, [string]$Exe = "D:\cargo-dbx-target\debug\dcx-host.exe")
$proc = Start-Process -FilePath $Exe -ArgumentList "--settings" -PassThru -WindowStyle Hidden
$deadline = (Get-Date).AddSeconds($Seconds)
while ((Get-Date) -lt $deadline) {
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    if ($null -eq $p) { Write-Host ("{0:HH:mm:ss.fff} gone" -f (Get-Date)); break }
    Write-Host ("{0:HH:mm:ss.fff} hwnd={1} threads={2}" -f (Get-Date), $p.MainWindowHandle, $p.Threads.Count)
    Start-Sleep -Milliseconds 400
}

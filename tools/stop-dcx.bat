@echo off
rem Stops the DCX host. The injected engine keeps running inside Explorer, so
rem the current taskbar look stays until Explorer restarts (or logoff).
rem To get the stock taskbar back: run this, then update-engine.bat.
setlocal
cd /d "%~dp0"

echo [1/2] stopping dcx-host and its standalone apps ...
rem taskkill /f bypasses the host's graceful shutdown, so apps::stop_all()
rem never runs. Stop every standalone app explicitly, or the box / indexer /
rem island keeps running with no host (and no injected engine watcher) behind it.
taskkill /f /im dcx-host.exe >nul 2>&1
taskkill /f /im dcx-box.exe >nul 2>&1
taskkill /f /im dcx-search.exe >nul 2>&1
taskkill /f /im dcx-island.exe >nul 2>&1

echo [2/2] done. dcx_engine.dll is still loaded in explorer.exe.
echo        Restart Explorer to unload it (update-engine.bat does that).
ping -n 3 127.0.0.1 >nul
exit /b 0

@echo off
rem Start the DCX host detached, without the settings window.
rem
rem Double-click this instead of dcx-host.exe when you only want the tray icon,
rem the taskbar theme and the file-box tile. The host starts enabled standalone
rem apps itself (dcx-box.exe / dcx-island.exe per %LOCALAPPDATA%\dcx\apps.toml);
rem do not launch them by hand. dcx-search.exe and dcx-island.exe are default-off:
rem search is started by Alt+S; the island must first be enabled in Settings ->
rem "Desktop Info Island" and is then toggled with Alt+I. The host is started by
rem cmd, so it keeps running after this window closes, just like the logon entry.
start "" "%~dp0dcx-host.exe" --no-settings

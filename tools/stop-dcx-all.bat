@echo off
rem Stop every DCX process and release the files in this folder.
rem
rem Use this before replacing dcx-*.exe / dcx_engine.dll by hand.
rem   stop-dcx-all.bat       ask before restarting Explorer
rem   stop-dcx-all.bat /y    restart Explorer without asking
rem                          (needed to unlock dcx_engine.dll)
rem
rem All Windows tools are called by absolute path: a Git/MSYS install in PATH
rem otherwise shadows find.exe and the status checks silently fail.
setlocal EnableExtensions EnableDelayedExpansion
cd /d "%~dp0"

set "SYS=%SystemRoot%\System32"
set "TASKKILL=%SYS%\taskkill.exe"
set "TASKLIST=%SYS%\tasklist.exe"
set "FINDSTR=%SYS%\findstr.exe"
set "CHOICE=%SYS%\choice.exe"
set "PING=%SYS%\ping.exe"

set "HOST=dcx-host.exe"
set "BOX=dcx-box.exe"
set "SEARCH=dcx-search.exe"
set "INJECTOR=wh_injector.exe"
set "ENGINE=dcx_engine.dll"

echo ============================================================
echo  DCX - stop all processes and unlock this folder
echo ============================================================
echo.

echo [1/4] stopping host (tray + settings + watchdog) ...
"%TASKKILL%" /f /im "%HOST%" >nul 2>&1

echo [2/4] stopping standalone apps ...
"%TASKKILL%" /f /im "%BOX%" >nul 2>&1
"%TASKKILL%" /f /im "%SEARCH%" >nul 2>&1
"%TASKKILL%" /f /im "%INJECTOR%" >nul 2>&1

set /a tries=0
:wait_gone
set "left="
for %%I in ("%HOST%" "%BOX%" "%SEARCH%") do (
  "%TASKLIST%" /fi "imagename eq %%~I" 2>nul | "%FINDSTR%" /i /c:"%%~I" >nul && set "left=1"
)
if not defined left goto processes_gone
set /a tries+=1
if !tries! leq 3 (
  "%TASKKILL%" /f /im "%HOST%" >nul 2>&1
  "%TASKKILL%" /f /im "%BOX%" >nul 2>&1
  "%TASKKILL%" /f /im "%SEARCH%" >nul 2>&1
  "%PING%" -n 3 127.0.0.1 >nul
  goto wait_gone
)
echo [WARN] some DCX processes refused to exit - check Task Manager.
set "STOP_FAILED=1"
goto check_engine
:processes_gone
echo        all DCX processes are gone.

:check_engine
echo.
echo [3/4] checking %ENGINE% ...
set "ENGINE_BUSY="
"%TASKLIST%" /m "%ENGINE%" 2>nul | "%FINDSTR%" /i /c:"%ENGINE%" >nul && set "ENGINE_BUSY=1"
if not defined ENGINE_BUSY (
  echo        not loaded - all files in this folder are unlocked.
  goto final
)
echo        still loaded inside explorer.exe.
echo        Restarting Explorer is required to unlock it.
if /i "%~1"=="/y" (
  echo        /y given - restarting Explorer.
  goto restart_shell
)
echo.
"%CHOICE%" /c YN /n /m "Restart Explorer now to unlock %ENGINE%? [Y/N] "
if errorlevel 2 goto engine_kept

:restart_shell
echo [4/4] restarting Explorer ...
"%TASKKILL%" /f /im explorer.exe >nul 2>&1
"%PING%" -n 3 127.0.0.1 >nul
set /a tries=0
:wait_shell
"%TASKLIST%" /fi "imagename eq explorer.exe" 2>nul | "%FINDSTR%" /i /c:"explorer.exe" >nul
if not errorlevel 1 goto shell_up
set /a tries+=1
if !tries! gtr 10 (
  echo        Explorer did not come back - starting it ...
  start "" explorer.exe
  "%PING%" -n 3 127.0.0.1 >nul
  goto shell_up
)
"%PING%" -n 2 127.0.0.1 >nul
goto wait_shell
:shell_up
echo        Explorer is back.
set "ENGINE_BUSY="
"%TASKLIST%" /m "%ENGINE%" 2>nul | "%FINDSTR%" /i /c:"%ENGINE%" >nul && set "ENGINE_BUSY=1"
if defined ENGINE_BUSY (
  echo [WARN] %ENGINE% is still reported as loaded; log off/on and retry.
) else (
  echo        %ENGINE% is unlocked.
)
goto final

:engine_kept
echo        Explorer left running - only the .exe files are unlocked.
echo        Do not replace %ENGINE% until Explorer is restarted.

:final
echo.
if defined STOP_FAILED (
  echo [WARN] DCX is NOT fully stopped - close the remaining processes
  echo        in Task Manager before replacing files.
) else (
  echo DCX processes are stopped.
)
if defined ENGINE_BUSY (
  echo [WARN] %ENGINE% is still in use by Explorer; do not replace it
  echo        until Explorer has been restarted.
) else (
  echo All files in this folder are unlocked.
)
echo.
echo Replace files in:
echo   %~dp0
echo Start it again with start-dcx.bat
echo.
pause
exit /b 0

@echo off
rem Double-click me: runs diagnose.ps1 regardless of the execution policy
rem (a .ps1 downloaded from the internet is blocked by default).
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0diagnose.ps1"
echo.
pause

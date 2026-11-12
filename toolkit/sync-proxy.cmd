@echo off
setlocal
cd /d "%~dp0"

if not exist "%~dp0sync-proxy.ps1" (
  echo [ERROR] sync-proxy.ps1 not found in:
  echo         %~dp0
  echo Keep sync-proxy.cmd and sync-proxy.ps1 in the same folder.
  echo.
  pause
  exit /b 1
)

powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0sync-proxy-wizard.ps1"

echo.
pause

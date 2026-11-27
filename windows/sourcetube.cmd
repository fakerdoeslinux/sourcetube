@echo off
setlocal
cd /d "%~dp0app"

where node >nul 2>nul
if errorlevel 1 (
  echo Node.js 20 or newer is required.
  echo Download it from https://nodejs.org and install it, then run this again.
  pause
  exit /b 1
)

for /f "tokens=2 delims= " %%v in ('node -v') do set NODEVER=%%v
echo SourceTube starting with Node %NODEVER%
echo Open http://localhost:8787 in your browser.
node server.mjs
pause

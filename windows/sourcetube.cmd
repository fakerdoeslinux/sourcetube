@echo off
setlocal
cd /d "%~dp0app"

if "%PORT%"=="" set PORT=8787
if "%APP_ORIGIN%"=="" set APP_ORIGIN=http://localhost:%PORT%
if "%GOOGLE_REDIRECT_URI%"=="" set GOOGLE_REDIRECT_URI=%APP_ORIGIN%/api/auth/callback
set SOURCETUBE_PUBLIC=%~dp0app\public
set SOURCETUBE_PLAYER=%~dp0app\sourcetube-player.exe

echo SourceTube running at %APP_ORIGIN%
start "SourceTube" /MIN "%~dp0app\sourcetube.exe"
timeout /t 2 >nul
start "" "%APP_ORIGIN%"

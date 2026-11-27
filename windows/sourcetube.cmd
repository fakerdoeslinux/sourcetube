@echo off
setlocal EnableDelayedExpansion
cd /d "%~dp0app"

rem ---- data directory ----------------------------------------------------------
if defined LOCALAPPDATA (
  set "SOURCETUBE_DATA=%LOCALAPPDATA%\SourceTube"
) else (
  set "SOURCETUBE_DATA=%~dp0app\data"
)
if not exist "%SOURCETUBE_DATA%" mkdir "%SOURCETUBE_DATA%" >nul 2>&1

rem ---- port file location ------------------------------------------------------
if defined TEMP (
  set "PORT_FILE=%TEMP%\sourcetube-port"
) else if defined TMP (
  set "PORT_FILE=%TMP%\sourcetube-port"
) else (
  set "PORT_FILE=%SOURCETUBE_DATA%\sourcetube-port"
)
if exist "%PORT_FILE%" del "%PORT_FILE%" >nul 2>&1

rem ---- env defaults ------------------------------------------------------------
if not defined PORT set PORT=8787
if not defined APP_ORIGIN set APP_ORIGIN=http://localhost:%PORT%
if not defined GOOGLE_REDIRECT_URI set GOOGLE_REDIRECT_URI=%APP_ORIGIN%/api/auth/callback
set "SOURCETUBE_PUBLIC=%~dp0app\public"
set "SOURCETUBE_PLAYER=%~dp0app\sourcetube-player.exe"

rem ---- start server ------------------------------------------------------------
set "LOG_FILE=%SOURCETUBE_DATA%\server.log"
start "SourceTube Server" /MIN "%~dp0app\sourcetube.exe" > "%LOG_FILE%" 2>&1

rem ---- wait for port file (up to 10 seconds) -----------------------------------
set BOUND_PORT=
for /L %%i in (1,1,10) do (
  if exist "%PORT_FILE%" (
    set /p BOUND_PORT=<"%PORT_FILE%"
    goto :open_browser
  )
  ping -n 2 127.0.0.1 >nul
)

:open_browser
if defined BOUND_PORT (
  set "OPEN_URL=http://localhost:!BOUND_PORT!"
) else (
  set "OPEN_URL=%APP_ORIGIN%"
)

if not "%SOURCETUBE_NO_BROWSER%"=="1" start "" "!OPEN_URL!"
endlocal

@echo off
setlocal DisableDelayedExpansion
if not defined LOCALAPPDATA (
  echo LOCALAPPDATA is missing. Run talos-server with an explicit --state-dir.
  pause
  exit /b 1
)
set "TALOS_LAUNCHER=%~dp0bin\windows-x86_64\talos-server-UNSIGNED.exe"
set "TALOS_STATE=%LOCALAPPDATA%\Talos\Server"
if not "%~1"=="" goto command
echo Talos local evaluation. Docker Desktop must be running with Linux containers.
echo Data is retained in "%TALOS_STATE%" and the Docker PostgreSQL volume.
"%TALOS_LAUNCHER%" --state-dir "%TALOS_STATE%" quickstart --config "%~dp0community-install.local.json"
if errorlevel 1 goto failed
echo Open https://talos.localhost:8443 and create your first account.
echo Review the local certificate guidance in GETTING_STARTED.md.
start "" "https://talos.localhost:8443"
pause
exit /b 0
:command
"%TALOS_LAUNCHER%" --state-dir "%TALOS_STATE%" %*
exit /b %errorlevel%
:failed
echo Talos could not start. Keep the error above and consult GETTING_STARTED.md.
pause
exit /b 1

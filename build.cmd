@echo off
REM OnionRouter — one-shot build (companion + XPI + installer)
REM Usage:   build.cmd  [--skip-installer]  [--debug]
REM Output:  installer\build\OnionRouter-Setup-<ver>.exe
REM          installer\build\onionrouter-<ver>.xpi

setlocal
set "ARGS="
:loop
if "%~1"=="" goto run
if /I "%~1"=="--skip-installer" set "ARGS=%ARGS% -SkipInstaller"
if /I "%~1"=="--debug"          set "ARGS=%ARGS% -DebugBuild"
shift
goto loop

:run
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\build.ps1"%ARGS%
exit /b %errorlevel%

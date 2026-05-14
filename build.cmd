@echo off
REM OnionRouter -- one-shot build (companion + XPI + installer)
REM Usage:   build.cmd  [--skip-installer]  [--debug]  [--skip-exclusion-check]
REM Output:  dist\OnionRouter-Setup-<ver>.exe
REM          dist\onionrouter-<ver>.xpi
REM
REM Prerequisite (one-time, per machine, as administrator):
REM   installer\windows\setup-defender-exclusion.ps1

setlocal
set "ARGS="
:loop
if "%~1"=="" goto run
if /I "%~1"=="--skip-installer"        set "ARGS=%ARGS% -SkipInstaller"
if /I "%~1"=="--debug"                 set "ARGS=%ARGS% -DebugBuild"
if /I "%~1"=="--skip-exclusion-check"  set "ARGS=%ARGS% -SkipExclusionCheck"
shift
goto loop

:run
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\build.ps1"%ARGS%
exit /b %errorlevel%

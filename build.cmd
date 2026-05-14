@echo off
REM OnionRouter -- one-shot build (companion + XPI + installer)
REM
REM Usage:   build.cmd  [--skip-installer]  [--debug]
REM Output:  dist\OnionRouter-Setup-<ver>.exe
REM          dist\onionrouter-<ver>.xpi
REM
REM Prefer running the GitHub Actions release workflow (push a tag)
REM over building locally -- it sidesteps Defender heuristics on
REM unsigned executables and produces reproducible artefacts.

setlocal
set "ARGS="
:loop
if "%~1"=="" goto run
if /I "%~1"=="--skip-installer"  set "ARGS=%ARGS% -SkipInstaller"
if /I "%~1"=="--debug"           set "ARGS=%ARGS% -DebugBuild"
shift
goto loop

:run
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0installer\build.ps1"%ARGS%
set "EXITCODE=%errorlevel%"
if not "%EXITCODE%"=="0" (
    echo.
    echo Build failed with exit code %EXITCODE%.
    pause
)
exit /b %EXITCODE%

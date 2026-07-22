@echo off
setlocal
set "APP=%~dp0MusicLinkFinder.exe"

if /i "%~1"=="--check" (
    echo Launcher OK
    exit /b 0
)

if not exist "%APP%" (
    echo MusicLinkFinder.exe was not found.
    pause
    exit /b 1
)

start "" "%APP%"
endlocal

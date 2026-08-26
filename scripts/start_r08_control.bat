@echo off
if /I not "%~1"=="__inner" (
    "%ComSpec%" /d /k call "%~f0" __inner
    exit /b
)

chcp 65001 >nul
cd /d "%~dp0.."

if not exist "target\release\r08.exe" (
    echo Building r08...
    cargo build -p r08 --release
    if errorlevel 1 (
        echo Build failed. Install https://rustup.rs first.
        pause
        exit /b 1
    )
)

echo Close the phone ring app and turn off phone Bluetooth first.
echo Injection starts after CONTROL_READY. Press Enter in this window to stop.
echo.
target\release\r08.exe control --touch-type 2 --sleep-minutes 1 --scroll-gain 4
echo.
echo Controller stopped.
echo Full log: %CD%\r08-control-latest.log
echo This window stays open. Type exit and press Enter to close it.

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

echo Disabling the R08 touch HID mode...
target\release\r08.exe disable-touch --sleep-minutes 1
echo.
echo R08 touch mode disable command sent. Type exit and press Enter to close this window.

@echo off
if /I not "%~1"=="__admin" (
    powershell.exe -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs -ArgumentList '__admin'"
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
echo Administrator mode protects the pointer by disabling only the R08 mouse child.
echo Controller is standing by. Double-tap the ring to enable control for one minute.
echo Press Enter or Ctrl+C to exit safely.
echo.
target\release\r08.exe
echo.
echo Controller stopped.
echo Full log: %CD%\r08-control-latest.log
pause

@echo off
if /I not "%~1"=="__admin" (
    powershell.exe -NoProfile -Command "Start-Process -FilePath '%~f0' -Verb RunAs -ArgumentList '__admin'"
    exit /b
)

chcp 65001 >nul
cd /d "%~dp0.."

echo Checking and building the current r08 source...
cargo build -p r08 --release --bin r08
if errorlevel 1 (
    echo Build failed. Install https://rustup.rs first.
    pause
    exit /b 1
)

echo Close the phone ring app and turn off phone Bluetooth first.
echo Administrator mode protects the pointer by disabling only the R08 mouse child.
echo Controller is standing by. Double-tap the ring to enable control for one minute.
echo When copying or pasting, the controller minimizes itself if this console is still in front.
echo Press Enter or Ctrl+C to exit safely.
echo.
target\release\r08.exe
echo.
echo Controller stopped.
echo Full log: %CD%\r08-control-latest.log
pause

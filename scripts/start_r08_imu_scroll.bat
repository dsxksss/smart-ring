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
echo v10/v11 use capacitive touch swipes for native wheel-only HID and never move the pointer.
echo v11 filters calibration and release samples for slower, monotonic scrolling.
echo Older firmware is rejected by touch-scroll-only mode.
echo Press Enter or Ctrl+C to stop safely.
echo.
set "RUST_LOG=info"
target\release\r08.exe imu-stream --acknowledge-unverified-candidate --inject --double-tap-wake --touch-scroll-only --seconds 0 --gain 0.2 --full-speed 60
echo.
echo IMU scroll stopped. No controller remains running.
echo Full log: %CD%\r08-control-latest.log
pause

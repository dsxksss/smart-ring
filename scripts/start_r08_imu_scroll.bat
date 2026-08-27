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
echo v9 uses the capacitive touch area for double-tap wake and blocks mouse reports in firmware.
echo Older firmware falls back to host IMU double-knock detection.
echo After IMU_CONTROL_AWAKE, keep the ring still for one second, then rotate it to scroll.
echo While awake, touch double-tap copies and triple-tap pastes.
echo Press Enter or Ctrl+C to stop safely.
echo.
set "RUST_LOG=info"
target\release\r08.exe imu-stream --acknowledge-unverified-candidate --inject --double-tap-wake --seconds 0 --gain 0.2 --full-speed 60
echo.
echo IMU scroll stopped. No controller remains running.
echo Full log: %CD%\r08-control-latest.log
pause

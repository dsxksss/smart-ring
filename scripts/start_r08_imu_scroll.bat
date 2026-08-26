@echo off
if /I not "%~1"=="__inner" (
    "%ComSpec%" /d /k call "%~f0" __inner
    exit /b
)

chcp 65001 >nul
cd /d "%~dp0.."

if not exist "target\release\r08.exe" (
    echo Building r08...
    cargo build -p r08 --release --bins
    if errorlevel 1 (
        echo Build failed. Install https://rustup.rs first.
        pause
        exit /b 1
    )
)

echo Close the phone ring app and turn off phone Bluetooth first.
echo Keep the ring still for about one second, then rotate it to scroll.
echo Press Enter or Ctrl+C to stop safely.
echo.
set "RUST_LOG=info"
target\release\r08.exe imu-stream --acknowledge-unverified-candidate --inject --gain 0.2 --full-speed 60
echo.
echo IMU scroll stopped. No controller remains running.
echo Full log: %CD%\r08-control-latest.log

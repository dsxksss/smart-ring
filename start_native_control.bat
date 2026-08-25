@echo off
if /I not "%~1"=="__inner" (
    "%ComSpec%" /d /k call "%~f0" __inner
    exit /b
)

chcp 65001 >nul
cd /d "%~dp0"

set "R08_DLL=native_ble\bin\Release\net10.0\R08NativeCli.dll"
if not exist "%R08_DLL%" (
    echo ERROR: Native controller was not built.
    echo Run build_native_control.bat first.
    pause
    exit /b 1
)

echo Close the phone ring app and turn off phone Bluetooth first.
echo Connecting to R08_9C07. Switch to the target app after CONTROL_READY.
echo.
dotnet "%R08_DLL%" --control --touch-type 2 --sleep-minutes 1 --scroll-gain 4
set "R08_EXIT_CODE=%ERRORLEVEL%"

echo.
echo Controller stopped. Exit code: %R08_EXIT_CODE%
echo Full log: %CD%\r08-control-latest.log
echo This window stays open. Type exit and press Enter to close it.

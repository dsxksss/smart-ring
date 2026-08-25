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
    exit /b 1
)

echo Disabling the R08 touch HID mode...
dotnet "%R08_DLL%" --listen --seconds 1 --touch-type 0 --sleep-minutes 1
echo.
echo R08 touch mode is now disabled. Type exit and press Enter to close this window.

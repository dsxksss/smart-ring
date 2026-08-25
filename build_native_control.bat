@echo off
chcp 65001 >nul
cd /d "%~dp0"

dotnet build "native_ble\R08NativeCli.csproj" -c Release --configfile "native_ble\NuGet.Config"
if errorlevel 1 (
    echo.
    echo Build failed. Send the error output to Codex.
    pause
    exit /b 1
)

echo.
echo Build succeeded. Run start_native_control.bat now.
pause

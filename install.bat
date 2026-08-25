@echo off
chcp 65001 >nul
cd /d "%~dp0"
echo 正在安装智能戒指检测器依赖...
python -m pip install -r requirements.txt
if errorlevel 1 (
  echo.
  echo 安装失败。请检查网络连接和 Python 安装。
  pause
  exit /b 1
)
echo.
echo 安装完成。现在可以双击 start.bat。
pause

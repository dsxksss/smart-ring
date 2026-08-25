@echo off
chcp 65001 >nul
cd /d "%~dp0"
python smart_ring_detector.py
if errorlevel 1 pause

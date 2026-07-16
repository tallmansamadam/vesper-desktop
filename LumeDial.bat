@echo off
rem Launch the LumeDial desktop app with no console window.
cd /d "%~dp0"
start "" pythonw lifx_gui.py

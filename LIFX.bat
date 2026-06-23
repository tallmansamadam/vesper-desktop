@echo off
rem Launch the LIFX desktop control app with no console window.
cd /d "%~dp0"
start "" pythonw lifx_gui.py

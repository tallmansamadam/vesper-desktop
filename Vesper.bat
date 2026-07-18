@echo off
rem Launch the Vesper desktop app with no console window.
cd /d "%~dp0"
start "" pythonw lifx_gui.py

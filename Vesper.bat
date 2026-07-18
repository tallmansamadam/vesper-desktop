@echo off
rem Launch the Vesper desktop app (native build; falls back to Python).
cd /d "%~dp0"
if exist "native\target\release\vesper.exe" (
    start "" "native\target\release\vesper.exe"
) else (
    start "" pythonw lifx_gui.py
)

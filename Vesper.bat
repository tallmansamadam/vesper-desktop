@echo off
rem Launch Vesper: the installed copy if present, else this working tree's
rem build, else the original Python app.
set "INSTALLED=%LOCALAPPDATA%\Programs\Vesper\vesper.exe"
if exist "%INSTALLED%" (
    start "" "%INSTALLED%"
    exit /b
)
cd /d "%~dp0"
if exist "native\target\release\vesper.exe" (
    start "" "native\target\release\vesper.exe"
) else (
    start "" pythonw lifx_gui.py
)

@echo off
:: stax — Windows launcher (double-click me)
:: Delegates to run.ps1 with ExecutionPolicy bypass so no prior PS config is needed.
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0run.ps1"
if %ERRORLEVEL% neq 0 (
    echo.
    echo [stax] Exited with error code %ERRORLEVEL%.
    pause
)

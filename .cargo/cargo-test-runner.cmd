@echo off
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "%~dp0cargo-test-runner.ps1" %*
exit /b %errorlevel%

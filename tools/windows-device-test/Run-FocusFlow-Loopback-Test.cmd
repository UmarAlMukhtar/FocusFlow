@echo off
cd /d "%~dp0"
"%~dp0focusflow-windows-device-test.exe"
set "test_exit=%errorlevel%"
echo.
echo Test exit code: %test_exit%  (0: finished or cancelled; 69: Unavailable; 71: Failed)
echo Read the console result and local result.json; cancellation is not PASS.
pause
exit /b %test_exit%

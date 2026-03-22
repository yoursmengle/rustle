@echo off
setlocal

set "MSVC_LINKER=C:\Program Files (x86)\Microsoft Visual Studio\2022\BuildTools\VC\Tools\MSVC\14.41.34120\bin\Hostx64\x64\link.exe"

if exist "%MSVC_LINKER%" goto run_linker

echo [rustle] MSVC linker not found: %MSVC_LINKER% 1>&2
exit /b 1

:run_linker
"%MSVC_LINKER%" %*
exit /b %ERRORLEVEL%

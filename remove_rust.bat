@echo off
set "BASE_DIR=D:\Dev\Rust"
set "BIN_PATH=%BASE_DIR%\.cargo\bin"

echo WARNING: About to permanently delete %BASE_DIR% and remove related environment variables!
choice /c YN /m "Confirm uninstall?"
if errorlevel 2 exit

rmdir /s /q "%BASE_DIR%"

powershell -Command ^
"[Environment]::SetEnvironmentVariable('RUSTUP_HOME',$null,'User'); ^
[Environment]::SetEnvironmentVariable('CARGO_HOME',$null,'User'); ^
$old=[Environment]::GetEnvironmentVariable('PATH','User'); ^
$new=($old -split ';') | Where-Object {$_ -notlike '*D:\Dev\Rust\.cargo\bin*'}; ^
[Environment]::SetEnvironmentVariable('PATH',($new -join ';'),'User')"

echo Uninstallation complete and cleaned up!
pause
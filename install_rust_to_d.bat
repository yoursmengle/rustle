@echo off
:: ======================================================
:: Rust one-click install to D:\Dev\Rust (persistent envvars + mirror fallback)
:: Updated: 2025-12
:: ======================================================
setlocal enabledelayedexpansion
set "BASE_DIR=D:\Dev\Rust"
set "RUSTUP_HOME=%BASE_DIR%\.rustup"
set "CARGO_HOME=%BASE_DIR%\.cargo"
set "GLOBAL_TARGET_DIR=%BASE_DIR%\target"

:: Preferred domestic mirror (USTC). You can change to another mirror if desired.
set "RUST_MIRROR=https://mirrors.ustc.edu.cn/rust-static"
set "RUSTUP_INIT_URL=%RUST_MIRROR%/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"

echo ================================================
echo Rust one-click install (persistent envvars + global target)
echo Install path: %BASE_DIR%
echo Mirror: %RUST_MIRROR%
echo ================================================

mkdir "%BASE_DIR%" 2>nul
mkdir "%GLOBAL_TARGET_DIR%" 2>nul

:: Persistently set environment variables (include mirror so updates use it)
powershell -Command ^
"[Environment]::SetEnvironmentVariable('RUSTUP_HOME', '%RUSTUP_HOME%', 'User'); ^
[Environment]::SetEnvironmentVariable('CARGO_HOME', '%CARGO_HOME%', 'User'); ^
[Environment]::SetEnvironmentVariable('RUSTUP_DIST_SERVER', '%RUST_MIRROR%', 'User'); ^
[Environment]::SetEnvironmentVariable('RUSTUP_UPDATE_ROOT', '%RUST_MIRROR%/rustup', 'User'); ^
Write-Host 'Persisted RUSTUP_HOME, CARGO_HOME, and mirror envs'"

:: Download rustup-init.exe (mirror first, then official fallback)
set "TEMP_DIR=%TEMP%\rust_install_final"
rmdir /s /q "%TEMP_DIR%" 2>nul
mkdir "%TEMP_DIR%"

echo Downloading rustup-init from official server first...
powershell -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13; Invoke-WebRequest -Uri 'https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe' -OutFile '%TEMP_DIR%\rustup-init.exe' -UseBasicParsing -TimeoutSec 60" 2>nul

if not exist "%TEMP_DIR%\rustup-init.exe" (
  echo Official server download failed; attempting USTC mirror...
  powershell -Command "[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12 -bor [Net.SecurityProtocolType]::Tls13; Invoke-WebRequest -Uri '%RUSTUP_INIT_URL%' -OutFile '%TEMP_DIR%\rustup-init.exe' -UseBasicParsing -TimeoutSec 60" 2>nul
)

if not exist "%TEMP_DIR%\rustup-init.exe" (
  echo ERROR: rustup-init.exe could not be downloaded from either source.
  echo Please check your network/proxy settings and try again.
  pause
  exit /b 1
)

for %%I in ("%TEMP_DIR%\rustup-init.exe") do set "RUSTUP_DL_SIZE=%%~zI"
echo Downloaded rustup-init.exe size: %RUSTUP_DL_SIZE% bytes

if "%RUSTUP_DL_SIZE%"=="0" (
  echo ERROR: rustup-init.exe downloaded as 0 bytes. Check mirror or retry.
  pause
  exit /b 1
)

echo Running rustup-init (with --no-verify to skip signature validation on mirrors)...
"%TEMP_DIR%\rustup-init.exe" -y --default-toolchain none --no-modify-path --no-verify
if errorlevel 1 (
  echo WARNING: rustup-init exited with code %errorlevel%. Trying without --no-verify...
  "%TEMP_DIR%\rustup-init.exe" -y --default-toolchain none --no-modify-path
  if errorlevel 1 (
    echo ERROR: rustup-init.exe failed.
    pause
    exit /b %errorlevel%
  )
)

:: Ensure PATH includes cargo bin for this session's commands
set "BIN_PATH=%CARGO_HOME%\bin"
set "PATH=%BIN_PATH%;%PATH%"

:: Add PATH persistently for future shells
powershell -Command "$p=[Environment]::GetEnvironmentVariable('PATH','User'); if($p -notlike '*%BIN_PATH%*'){[Environment]::SetEnvironmentVariable('PATH',$p+';%BIN_PATH%','User')}"

:: Configure global target dir to avoid per-project target duplication
(
echo [build]
echo target-dir = "%GLOBAL_TARGET_DIR:\=\\%"
echo.
echo [profile.dev]
echo incremental = true
echo.
echo [profile.release]
echo incremental = true
) > "%CARGO_HOME%\config.toml"

:: Install toolchain robustly: try mirror (3 attempts), then official
if exist "%CARGO_HOME%\bin\rustup.exe" (
  set "RUSTUP_EXE=%CARGO_HOME%\bin\rustup.exe"
) else (
  set "RUSTUP_EXE=rustup"
)

echo Installing default toolchain (stable-x86_64-pc-windows-msvc) with mirror+fallback...
set attempt=1
:try_mirror
echo Trying mirror %RUST_MIRROR% (attempt %attempt%)...
set "RUSTUP_DIST_SERVER=%RUST_MIRROR%"
set "RUSTUP_UPDATE_ROOT=%RUST_MIRROR%/rustup"
"%RUSTUP_EXE%" toolchain install stable-x86_64-pc-windows-msvc
if %errorlevel%==0 goto toolchain_ok
set /a attempt+=1
if %attempt% leq 3 (
  echo Waiting before retrying...
  timeout /t 3 >nul
  goto try_mirror
)

echo Mirror attempts failed — trying official server...
set "RUSTUP_DIST_SERVER=https://static.rust-lang.org"
set "RUSTUP_UPDATE_ROOT=https://static.rust-lang.org/rustup"
"%RUSTUP_EXE%" toolchain install stable-x86_64-pc-windows-msvc
if %errorlevel% neq 0 (
  echo ERROR: toolchain install failed using both mirror and official server.
  echo Please check network/proxy settings or try again later.
  pause
  exit /b 1
)

:toolchain_ok
echo Toolchain installed successfully.
if exist "%CARGO_HOME%\bin\rustc.exe" (
  "%CARGO_HOME%\bin\rustc.exe" --version
) else (
  echo Note: rustc executable not found in %CARGO_HOME%\bin yet. Open a new shell after installation to pick up PATH.
)

echo ================================================
echo Installation complete^! All project build artifacts will go to %GLOBAL_TARGET_DIR%
echo ================================================
rmdir /s /q "%TEMP_DIR%" 2>nul
pause
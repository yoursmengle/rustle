<#
PowerShell script to configure Chinese mirrors for Rust (rustup/cargo) and MSYS2 (pacman).
Run in PowerShell as: .\scripts\setup_china_mirrors.ps1 [-Persist]

- Sets env vars for this session to use USTC mirrors for rustup
- Optionally writes env vars to user environment (with -Persist)
- Ensures `.cargo/config.toml` uses USTC as registry replacement
- Updates MSYS2 pacman mirrorlists to include USTC + TUNA mirrors (backups created)
#>

param(
  [switch]$Persist,
  [switch]$Test
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "[mirrors] $msg" -ForegroundColor Cyan }
function Write-Warn($msg) { Write-Warning "[mirrors] $msg" }

# Rustup mirrors (USTC)
$rustupDist = 'https://mirrors.ustc.edu.cn/rust-static'
$rustupUpdate = 'https://mirrors.ustc.edu.cn/rust-static/rustup'

Write-Step "Setting rustup env vars for this session"
$env:RUSTUP_DIST_SERVER = $rustupDist
$env:RUSTUP_UPDATE_ROOT = $rustupUpdate
Write-Host "RUSTUP_DIST_SERVER=$env:RUSTUP_DIST_SERVER"
Write-Host "RUSTUP_UPDATE_ROOT=$env:RUSTUP_UPDATE_ROOT"

# Helper to test rustup mirror reachability
function Test-RustupMirror() {
  Write-Step "Testing rustup mirror reachability: $rustupUpdate"
  try {
    $r = Invoke-WebRequest -Uri "$rustupUpdate/channel-rust-stable.toml" -Method Head -UseBasicParsing -TimeoutSec 15
    if ($r.StatusCode -ge 200 -and $r.StatusCode -lt 400) { Write-Host "OK: mirror reachable" -ForegroundColor Green; return $true }
    else { Write-Warn "Unexpected status: $($r.StatusCode)"; return $false }
  } catch {
    Write-Warn "Mirror test failed: $($_.Exception.Message)"; return $false
  }
}

if ($Persist) {
  Write-Step "Persisting rustup env vars to user environment (User scope)"
  # Use the .NET API which is more reliable than setx for Unicode and quoting
  [Environment]::SetEnvironmentVariable('RUSTUP_DIST_SERVER', $rustupDist, 'User')
  [Environment]::SetEnvironmentVariable('RUSTUP_UPDATE_ROOT', $rustupUpdate, 'User')
  Write-Host "Persisted (applies to newly opened shells)."
}

# Also write global cargo config in the user's profile so all projects use the mirror
$userCargoDir = Join-Path $env:USERPROFILE '.cargo'
if (-not (Test-Path $userCargoDir)) { New-Item -Path $userCargoDir -ItemType Directory -Force | Out-Null }
$userCargoCfg = Join-Path $userCargoDir 'config.toml'
if (-not (Test-Path $userCargoCfg)) { New-Item -Path $userCargoCfg -ItemType File -Force | Out-Null }

$userContent = Get-Content $userCargoCfg -Raw
$userDesired = @'
[net]
# Use git HTTP fetcher which can use system curl/proxy if present
git-fetch-with-cli = true

[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "https://mirrors.ustc.edu.cn/crates.io-index"
'@
if ($userContent -notmatch 'mirrors\.ustc\.edu\.cn') {
  Write-Step "Updating user cargo config at $userCargoCfg to use USTC registry mirror"
  Set-Content -Path $userCargoCfg -Value $userDesired -Encoding UTF8
} else {
  Write-Step "User cargo config already configured for USTC"
}

# Cargo registry: ensure .cargo/config.toml uses USTC
$cargoCfg = Join-Path (Get-Location) '.cargo\config.toml'
if (-not (Test-Path $cargoCfg)) {
  Write-Step "Creating $cargoCfg"
  New-Item -Path $cargoCfg -ItemType File -Force | Out-Null
}

# Read current and ensure content contains ustc registry
$content = Get-Content $cargoCfg -Raw
$desired = @'
[build]
# Use GNU target by default only if you want mingw builds. Leave unset otherwise.
# target = "x86_64-pc-windows-gnu"

[net]
git-fetch-with-cli = true

[source.crates-io]
replace-with = "ustc"

[source.ustc]
registry = "https://mirrors.ustc.edu.cn/crates.io-index"
'@

if ($content -notmatch 'mirrors\.ustc\.edu\.cn') {
  Write-Step "Updating project cargo config at $cargoCfg to use USTC registry mirror"
  Set-Content -Path $cargoCfg -Value $desired -Encoding UTF8
} else {
  Write-Step "$cargoCfg already configured for USTC" 
}

# Optionally run a quick test of the rustup mirror if asked
if ($Test) {
  $ok = Test-RustupMirror
  if ($ok) { Write-Host "rustup mirror looks reachable. You can now run 'rustup update'" -ForegroundColor Green }
  else { Write-Warn "rustup mirror not reachable; try the alternate mirrors or check network/proxy settings." }
}

# MSYS2 pacman mirrors
$msysRoot = 'C:\msys64'
if (Test-Path $msysRoot) {
  Write-Step "MSYS2 detected at $msysRoot; updating pacman mirrorlists (backups created)"
  $mirrorFiles = @(
    '\\etc\\pacman.d\\mirrorlist',
    '\\etc\\pacman.d\\mirrorlist.mingw32',
    '\\etc\\pacman.d\\mirrorlist.mingw64',
    '\\etc\\pacman.d\\mirrorlist.ucrt64',
    '\\etc\\pacman.d\\mirrorlist.clang64'
  )

  foreach ($mf in $mirrorFiles) {
    $path = Join-Path $msysRoot $mf
    if (Test-Path $path) {
      $bak = "$path.bak"
      if (-not (Test-Path $bak)) { Copy-Item -Path $path -Destination $bak -Force }
      Write-Step "Updating $path"

      $entries = @(
        '# USTC mirror',
        'Server = https://mirrors.ustc.edu.cn/msys2/$repo/$arch',
        '# TUNA mirror',
        'Server = https://mirrors.tuna.tsinghua.edu.cn/msys2/$repo/$arch',
        '# SJTUG mirror (fallback)',
        'Server = https://mirrors.sjtug.sjtu.edu.cn/msys2/$repo/$arch'
      )

      # Use CRLF for Windows files
      $entries -join "`r`n" | Set-Content -Path $path -Encoding ASCII
    }
  }

  Write-Step "To refresh pacman database, run inside MSYS2: pacman -Syyu"
} else {
  Write-Warn "MSYS2 not found at $msysRoot; skipping pacman mirror setup"
}

Write-Step "Mirror setup complete. Recommended: restart shell (or sign out/in) and run 'cargo build' or 'rustup update' to validate."

# Optional test helper: run a small validation of reachability for the rustup update root
function Test-RustupMirror() {
  Write-Step "Testing rustup mirror reachability: $rustupUpdate"
  try {
    $r = Invoke-WebRequest -Uri "$rustupUpdate/channel-rust-stable.toml" -Method Head -UseBasicParsing -TimeoutSec 15
    if ($r.StatusCode -ge 200 -and $r.StatusCode -lt 400) { Write-Host "OK: mirror reachable" -ForegroundColor Green; return $true }
    else { Write-Warn "Unexpected status: $($r.StatusCode)"; return $false }
  } catch {
    Write-Warn "Mirror test failed: $($_.Exception.Message)"; return $false
  }
}

if ($Persist) {
  # advise how to reload env in current session
  Write-Step "Note: persistent env vars apply to new shells. To use them now in PowerShell:"
  Write-Host "  $env:RUSTUP_DIST_SERVER = '$rustupDist'" -ForegroundColor Yellow
  Write-Host "  $env:RUSTUP_UPDATE_ROOT = '$rustupUpdate'" -ForegroundColor Yellow
}

Write-Step "If you want the script to test rustup mirror reachability automatically, re-run with: .\scripts\setup_china_mirrors.ps1 -Persist -Test"
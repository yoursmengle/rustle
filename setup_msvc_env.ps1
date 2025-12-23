<#
Loads a working Rust (MSVC) build environment into the CURRENT PowerShell session.

Why this exists:
- Rust (x86_64-pc-windows-msvc) requires MSVC + Windows SDK link libraries.
- Git/MSYS2 ship a different `link.exe` that breaks Rust MSVC builds if it wins on PATH.

Run (PowerShell):
  .\setup_msvc_env.ps1

If your VS Build Tools install is incomplete, rerun with:
  .\setup_msvc_env.ps1 -InstallCppWorkload
#>

[CmdletBinding()]
param(
  [string]$RustBase = 'D:\Dev\Rust',
  [switch]$InstallCppWorkload
)

$ErrorActionPreference = 'Stop'

function Write-Step($msg) { Write-Host "[rustle-env] $msg" -ForegroundColor Cyan }
function Write-Warn($msg) { Write-Warning "[rustle-env] $msg" }

function Get-VsWherePath {
  $default = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (Test-Path $default) { return $default }
  $cmd = Get-Command vswhere.exe -ErrorAction SilentlyContinue
  if ($cmd) { return $cmd.Source }
  return $null
}

function Get-BuildToolsInstance([string]$vswherePath) {
  # Try several queries in order of preference, and prefer instances that contain the VC tools on disk.
  $queries = @(
    { & $vswherePath -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -format json },
    { & $vswherePath -latest -products * -format json },
    { & $vswherePath -all -products * -format json }
  )

  foreach ($q in $queries) {
    try {
      $raw = & $q
    } catch {
      $raw = $null
    }
    if (-not $raw) { continue }
    $json = $raw | ConvertFrom-Json -ErrorAction SilentlyContinue
    if (-not $json) { continue }

    # If array, pick an instance that actually has VC tools installed on disk
    $instances = @($json)
    foreach ($inst in $instances) {
      $root = ($inst.installationPath ?? '').Trim()
      if ($root -and (Test-Path (Join-Path $root 'VC\Tools\MSVC'))) {
        return $inst
      }
    }

    # otherwise return the first instance found
    return $instances | Select-Object -First 1
  }

  return $null
}

function Install-Or-Repair-CppWorkload([string]$vsInstallPath) {
  $setupExe = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\setup.exe'
  if (-not (Test-Path $setupExe)) {
    throw "Visual Studio Installer not found at: $setupExe"
  }

  Write-Step "Launching Visual Studio Installer to add C++ tools + Windows SDK (silent/repair mode)..."

  # Workload: Desktop development with C++
  # This is the broadest reliable option for Rust MSVC linking.
  $args = @(
    'modify',
    "--installPath", "`"$vsInstallPath`"",
    '--add', 'Microsoft.VisualStudio.Workload.VCTools',
    '--includeRecommended',
    '--passive',
    '--norestart'
  )

  & $setupExe @args | Out-Host
}

function Import-VsDevCmdEnv([string]$vsInstall) {
  $vsDevCmd = Join-Path $vsInstall 'Common7\Tools\VsDevCmd.bat'
  if (-not (Test-Path $vsDevCmd)) {
    throw "VsDevCmd.bat not found at: $vsDevCmd"
  }

  Write-Step "Loading MSVC environment from: $vsDevCmd"

  $cmdLine = "call `"$vsDevCmd`" -no_logo -host_arch=x64 -arch=x64 && set"
  $envDump = & cmd.exe /s /c $cmdLine

  $allow = @(
    'Path','INCLUDE','LIB','LIBPATH',
    'VSINSTALLDIR','VCINSTALLDIR','VCToolsInstallDir','VCToolsVersion',
    'WindowsSdkDir','WindowsSDKVersion','WindowsSdkBinPath','WindowsSdkVerBinPath',
    'UniversalCRTSdkDir','UCRTVersion',
    'VSCMD_ARG_HOST_ARCH','VSCMD_ARG_TGT_ARCH'
  )

  foreach ($line in $envDump) {
    if ($line -notmatch '^(?<name>[^=]+)=(?<value>.*)$') { continue }
    $name = $matches['name']
    $value = $matches['value']

    # Skip cmd.exe pseudo-vars like '=C:'
    if ($name.StartsWith('=')) { continue }

    if ($allow -contains $name) {
      if ($name -ieq 'Path') { $env:Path = $value }
      else { Set-Item -Path "Env:$name" -Value $value }
    }
  }
}

function Normalize-PathList([string]$pathValue) {
  $parts = $pathValue -split ';' | ForEach-Object { $_.Trim() } | Where-Object { $_ -ne '' }
  $seen = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
  $out = New-Object 'System.Collections.Generic.List[string]'
  foreach ($p in $parts) {
    if ($seen.Add($p)) { $out.Add($p) | Out-Null }
  }
  return ,$out
}

function Fix-Linker-PathOrder {
  # Move Git/MSYS2 usr\bin to the end to avoid picking up their `link.exe`.
  $current = Normalize-PathList $env:Path
  $bad = New-Object 'System.Collections.Generic.List[string]'
  $rest = New-Object 'System.Collections.Generic.List[string]'
  foreach ($p in $current) {
    if ($p -match '\\Git\\usr\\bin$' -or $p -match '\\msys64\\usr\\bin$') { $bad.Add($p) | Out-Null }
    else { $rest.Add($p) | Out-Null }
  }
  $env:Path = (Normalize-PathList (($rest + $bad) -join ';')) -join ';'
}

function Setup-Rust([string]$rustBase) {
  $env:RUSTUP_HOME = Join-Path $rustBase '.rustup'
  $env:CARGO_HOME  = Join-Path $rustBase '.cargo'
  $cargoBin = Join-Path $env:CARGO_HOME 'bin'
  if (Test-Path $cargoBin) {
    $env:Path = "$cargoBin;$env:Path"
  } else {
    Write-Warn "Cargo bin not found at: $cargoBin"
  }
}

function Verify-Tools {
  Write-Step "Verifying tool resolution (first hit matters)..."

  $link = $null
  $cl = $null
  try { $link = (& where.exe link 2>$null) } catch { $link = $null }
  try { $cl = (& where.exe cl 2>$null) } catch { $cl = $null }

  if ($cl) { Write-Host "cl.exe -> $($cl | Select-Object -First 1)" -ForegroundColor Green }
  else { Write-Warn "cl.exe not found (C++ workload likely missing)" }

  if ($link) { Write-Host "link.exe -> $($link | Select-Object -First 1)" -ForegroundColor Green }
  else { Write-Warn "link.exe not found" }

  if ($link -and ($link | Select-Object -First 1) -match '\\Git\\usr\\bin\\link\.exe$') {
    Write-Warn "Git's link.exe is first in PATH. Rust MSVC builds will fail until MSVC link.exe is ahead (or Git usr\\bin is moved behind)."
  }
  if ($link -and ($link | Select-Object -First 1) -match '\\msys64\\usr\\bin\\link\.exe$') {
    Write-Warn "MSYS2's link.exe is first in PATH. Rust MSVC builds will fail until MSVC link.exe is ahead (or msys64 usr\\bin is moved behind)."
  }
}

Write-Step "Starting MSVC environment setup..."

$vswhere = Get-VsWherePath
if (-not $vswhere) {
  throw "vswhere.exe not found. Install Visual Studio Build Tools 2022 (includes vswhere), then rerun."
}

$instance = Get-BuildToolsInstance $vswhere
if (-not $instance) {
  throw "No Visual Studio/Build Tools instance found. Install Visual Studio Build Tools 2022, then rerun."
}

$vsInstall = ($instance.installationPath ?? '').Trim()
if (-not $vsInstall) {
  throw "vswhere returned an instance without an installationPath."
}

Write-Step "Detected: $($instance.displayName) at $vsInstall"
if ($instance.isComplete -ne $true) {
  Write-Warn "Build Tools install is not complete (installer was canceled or still running)."
  if ($InstallCppWorkload) {
    Install-Or-Repair-CppWorkload -vsInstallPath $vsInstall
  } else {
    Write-Warn "Run: .\setup_msvc_env.ps1 -InstallCppWorkload"
    Write-Warn "Or open 'Visual Studio Installer' -> Modify -> select 'Desktop development with C++'."
    return
  }
}

Import-VsDevCmdEnv -vsInstall $vsInstall
Fix-Linker-PathOrder
Setup-Rust -rustBase $RustBase
Verify-Tools

Write-Step "Environment loaded for this terminal session. Try: cargo build"

# One-step MSVC release build with self-contained env setup.
param(
  [switch]$Clean
)

$ErrorActionPreference = 'Stop'

function Step($msg) { Write-Host "[build] $msg" -ForegroundColor Cyan }
function Warn($msg) { Write-Warning "[build] $msg" }

$msvcTarget = 'x86_64-pc-windows-msvc'
$env:RUSTUP_TOOLCHAIN = "stable-$msvcTarget"

function Get-VsDevCmd {
  $vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
  if (-not (Test-Path $vswhere)) { throw 'vswhere.exe not found. Install Visual Studio Build Tools 2022 with C++ workload.' }
  $root = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
  if (-not $root) { throw 'VS Build Tools with Desktop development with C++ workload not found.' }
  $cmd = Join-Path $root 'Common7\Tools\VsDevCmd.bat'
  if (-not (Test-Path $cmd)) { throw "VsDevCmd.bat not found at $cmd" }
  return $cmd
}

function Import-VsEnv {
  param([string]$vsDevCmd)
  Step "Loading MSVC environment"
  $envDump = & cmd.exe /s /c "call `"$vsDevCmd`" -no_logo -host_arch=x64 -arch=x64 && set"
  foreach ($line in $envDump) {
    if ($line -notmatch '^(?<n>[^=]+)=(?<v>.*)$') { continue }
    $name = $matches['n']; $value = $matches['v']
    if ($name -eq 'Path') { $env:Path = $value }
    elseif ($name -in 'INCLUDE','LIB','LIBPATH','VCINSTALLDIR','VCToolsInstallDir','WindowsSdkDir','WindowsSDKVersion') {
      Set-Item -Path "Env:$name" -Value $value
    }
  }
}

function Ensure-CargoBin {
  $cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
  $cargoBin = Join-Path $cargoHome 'bin'
  if (Test-Path $cargoBin) { $env:Path = "$cargoBin;$env:Path" }
}

function Sanitize-Path {
  $bad = @('Git\usr\bin','msys64\usr\bin','TDM-GCC','mingw64\bin','mingw32\bin')
  $parts = $env:Path -split ';' | Where-Object { $_ }
  $good = @(); $rest = @()
  foreach ($p in $parts) {
    $norm = $p.ToLower()
    if ($bad | Where-Object { $norm -like "*$(($_).ToLower())*" }) { $rest += $p } else { $good += $p }
  }
  $env:Path = ($good + $rest) -join ';'
}

function Get-CargoTargetInfo {
  $json = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
  $targetDir = $json.target_directory
  $pkg = $json.packages | Select-Object -First 1
  $binName = ($pkg.targets | Where-Object { $_.kind -contains 'bin' } | Select-Object -First 1).name
  return @{ TargetDir = $targetDir; BinName = $binName }
}

$vsCmd = Get-VsDevCmd
Import-VsEnv -vsDevCmd $vsCmd
Ensure-CargoBin
Sanitize-Path

if ($Clean) {
  Step "cargo clean"
  & cargo clean
}

$buildArgs = @('build','--release','--target',$msvcTarget)
Step "cargo $($buildArgs -join ' ')"
& cargo @buildArgs

try {
  $info = Get-CargoTargetInfo
  $baseDir = Join-Path $info.TargetDir $msvcTarget
  $releaseDir = Join-Path $baseDir 'release'
  $exeName = "$($info.BinName).exe"
  $exePath = Join-Path $releaseDir $exeName

  Step "target_directory: $($info.TargetDir)"
  Step "release dir     : $releaseDir"
  Step "expected exe    : $exePath"

  if (Test-Path $exePath) {
    Step "Built executable: $exePath"
  } else {
    $fallback = Get-ChildItem -Path (Join-Path $releaseDir 'deps') -Filter "$($info.BinName)-*.exe" -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($fallback) {
      Copy-Item -Path $fallback.FullName -Destination $exePath -Force
      Step "Built executable (renamed from deps): $exePath"
    } else {
      Warn "No .exe files found in $releaseDir (build may have failed or target dir differs)."
    }
  }
    # Copy the built executable into the repository's target folder for easy access
    try {
      $repoTargetDir = Join-Path $PSScriptRoot 'target'
      $repoReleaseDir = Join-Path $repoTargetDir 'release'
      if (-not (Test-Path $repoReleaseDir)) { New-Item -ItemType Directory -Path $repoReleaseDir -Force | Out-Null }
      $destExe = Join-Path $repoReleaseDir $exeName
      if (Test-Path $exePath) {
        Copy-Item -Path $exePath -Destination $destExe -Force
        Step "Copied build artifact to: $destExe"
      }
    } catch {
      Warn "Failed to copy artifact into repo target dir: $($_.Exception.Message)"
    }
    # If Inno Setup is available, compile the installer using installer/RustleSetup.iss
    try {
      $issPath = Join-Path $PSScriptRoot 'installer\RustleSetup.iss'
      if (Test-Path $issPath) {
        # Try to find ISCC.exe (Inno Setup Compiler) in PATH or common install locations
        $isccCandidates = @()
        $cmd = Get-Command 'ISCC.exe' -ErrorAction SilentlyContinue
        if ($cmd) { $isccCandidates += $cmd.Source }
        $isccCandidates += (Join-Path ${env:ProgramFiles(x86)} 'Inno Setup 6\ISCC.exe')
        $isccCandidates += (Join-Path ${env:ProgramFiles} 'Inno Setup 6\ISCC.exe')

        $iscc = $isccCandidates | Where-Object { $_ -and (Test-Path $_) } | Select-Object -First 1
        if ($iscc) {
          Step "Found Inno Setup compiler: $iscc"
          $installerOut = Join-Path $PSScriptRoot 'target\installer'
          if (-not (Test-Path $installerOut)) { New-Item -ItemType Directory -Path $installerOut -Force | Out-Null }

          # Pass SourcePath preprocessor define so the ISS can find files relative to installer directory
          # The ISS expects {#SourcePath} to point at the installer folder, so it can use ..\target\release
          $sourcePath = (Join-Path $PSScriptRoot 'installer') + '\\'
          Step "Using SourcePath for ISCC: $sourcePath"
          Step "Running ISCC to build installer into: $installerOut"
          & "$iscc" "/O$installerOut" "/DSourcePath=$sourcePath" "$issPath"
          $exit = $LASTEXITCODE
          if ($exit -eq 0) {
            Step "Inno Setup compiled installer successfully"
            # Copy the produced installer (pick newest matching name)
            $artifact = Get-ChildItem -Path $installerOut -Filter 'rustle-setup*.exe' -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($artifact) {
              $destInst = Join-Path (Join-Path $PSScriptRoot 'target\release') $artifact.Name
              Copy-Item -Path $artifact.FullName -Destination $destInst -Force
              Step "Copied installer to: $destInst"
            }
          } else {
            Warn "Inno Setup (ISCC.exe) failed with exit code $exit"
          }
        } else {
          Warn 'ISCC.exe (Inno Setup Compiler) not found; skipping installer build. Install Inno Setup or add ISCC.exe to PATH.'
        }
      } else {
        Warn "Installer script not found at $issPath; skipping installer build."
      }
    } catch {
      Warn "Error while attempting to build installer: $($_.Exception.Message)"
    }
} catch {
  Warn "Failed to locate artifact: $($_.Exception.Message)"
}

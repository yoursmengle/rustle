# Use this script inside the VS Code terminal to load Rust toolchain env vars for this session.
# Run:  .\set_rust_env.ps1

$Base = 'D:\Dev\Rust'
$env:RUSTUP_HOME = "$Base\.rustup"
$env:CARGO_HOME  = "$Base\.cargo"

# Prepend cargo bin so it wins over any other cargo on PATH
$cargoBin = "$($env:CARGO_HOME)\bin"
if ($env:Path -notmatch [regex]::Escape($cargoBin)) {
	$env:Path = "$cargoBin;$env:Path"
}

# Optional: show the resolved paths
Write-Host "RUSTUP_HOME=$($env:RUSTUP_HOME)" -ForegroundColor Cyan
Write-Host "CARGO_HOME=$($env:CARGO_HOME)" -ForegroundColor Cyan
Write-Host "cargo path added: $($env:CARGO_HOME)\bin" -ForegroundColor Cyan

# Optional: verify cargo/rustc availability
$Cargo = Get-Command cargo -ErrorAction SilentlyContinue
$Rustc = Get-Command rustc -ErrorAction SilentlyContinue
if ($Cargo) { Write-Host "cargo: $($Cargo.Source)" -ForegroundColor Green } else { Write-Warning "cargo not found" }
if ($Rustc) { Write-Host "rustc: $($Rustc.Source)" -ForegroundColor Green } else { Write-Warning "rustc not found" }

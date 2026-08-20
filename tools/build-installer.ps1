# Builds the release binary and wraps it in the Inno Setup installer.
#   powershell -ExecutionPolicy Bypass -File tools\build-installer.ps1
# Output: dist\Quotty-Setup-<version>.exe
param([string]$Iscc = "D:\Programs\Inno Setup 6\ISCC.exe")

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
Set-Location $root

# Single source of truth for the version: Cargo.toml.
$version = (Select-String -Path "Cargo.toml" -Pattern '^version\s*=\s*"(.+)"' |
            Select-Object -First 1).Matches[0].Groups[1].Value
Write-Output "Quotty $version"

if (-not (Test-Path $Iscc)) { throw "Inno Setup not found: $Iscc" }

cargo build --release
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force -Path dist | Out-Null
& $Iscc "/DAppVersion=$version" "installer\quotty.iss"
if ($LASTEXITCODE -ne 0) { throw "ISCC failed" }

$setup = "dist\Quotty-Setup-$version.exe"
"{0}  {1:N2} MB" -f $setup, ((Get-Item $setup).Length / 1MB)

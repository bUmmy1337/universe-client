$ErrorActionPreference = "Stop"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw "Rust не найден. Установите rustup с https://rustup.rs/ и перезапустите PowerShell."
}

$ProjectDir = Split-Path -Parent $MyInvocation.MyCommand.Path
Set-Location $ProjectDir

Write-Host "Проверка исходников..." -ForegroundColor Cyan
cargo test

Write-Host "Сборка universe для Windows x64..." -ForegroundColor Cyan
rustup target add x86_64-pc-windows-msvc
cargo build --release --target x86_64-pc-windows-msvc

$DistDir = Join-Path $ProjectDir "dist"
New-Item -ItemType Directory -Force -Path $DistDir | Out-Null
Copy-Item -Force `
    (Join-Path $ProjectDir "target\x86_64-pc-windows-msvc\release\universe.exe") `
    (Join-Path $DistDir "universe.exe")

Write-Host "Готово: $DistDir\universe.exe" -ForegroundColor Green


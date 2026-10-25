# Nehir Windows kurulum paketini (NSIS, kullanıcı başına) üretir.
#   pwsh packaging/windows/build.ps1
# Gerekenler: Rust (MSVC), Node.js 20+, cargo-tauri (`cargo install tauri-cli --version "^2" --locked`).
# Çıktı: target\release\bundle\nsis\Nehir_<sürüm>_x64-setup.exe
$ErrorActionPreference = 'Stop'
$root = Resolve-Path (Join-Path $PSScriptRoot '..\..')
Set-Location $root

Write-Host '1/5 Arayüz'
Push-Location ui; npm ci; npm run build; Pop-Location

Write-Host '2/5 Tarayıcı eklentisi'
node extension\build.mjs

Write-Host '3/5 Native messaging host'
cargo build --release -p nehir-host
$triple = (rustc -vV | Select-String '^host: (.+)$').Matches[0].Groups[1].Value
$bin = Join-Path $root 'crates\nehir-app\binaries'
New-Item -ItemType Directory -Force $bin | Out-Null
# Tauri "externalBin" dosyaları hedef üçlüsüyle adlandırılmasını ister; kurulumda sade adla (nehir-native-host.exe)
# nehir.exe'nin yanına konur. Uygulama host'u orada arar.
Copy-Item (Join-Path $root 'target\release\nehir-native-host.exe') (Join-Path $bin "nehir-native-host-$triple.exe") -Force

Write-Host '4/5 Uygulama + kurulum paketi'
Push-Location crates\nehir-app
cargo tauri build --config (Join-Path $root 'packaging\windows\bundle.conf.json')
Pop-Location

Write-Host '5/5 Çıktı'
Get-ChildItem (Join-Path $root 'target\release\bundle\nsis') -Filter *.exe | Select-Object Name, @{n='MB'; e={[math]::Round($_.Length/1MB, 1)}}

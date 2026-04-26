$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

Write-Host "Reproducing published benchmark claim surfaces"
Write-Host ""
Write-Host "[1/4] Marketable session vs Python"
cargo run -q -p corvid-cli -- bench compare python --session 2026-04-17-marketable-session
Write-Host ""
Write-Host "[2/4] Marketable session vs JS"
cargo run -q -p corvid-cli -- bench compare js --session 2026-04-17-marketable-session
Write-Host ""
Write-Host "[3/4] Corrected session vs Python"
cargo run -q -p corvid-cli -- bench compare python --session 2026-04-17-corrected-session
Write-Host ""
Write-Host "[4/4] Corrected session vs JS"
cargo run -q -p corvid-cli -- bench compare js --session 2026-04-17-corrected-session

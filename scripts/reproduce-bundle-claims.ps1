$ErrorActionPreference = "Stop"
$root = Resolve-Path (Join-Path $PSScriptRoot "..")
Set-Location $root

Write-Host "Reproducing bundle verification claims"
Write-Host ""
Write-Host "[1/2] Happy-path bundle verification"
bash "examples/phase22_demo/verify.sh"
Write-Host ""
Write-Host "[2/2] Negative-path bundle verification"
foreach ($dir in @("failing_hash","failing_signature","failing_rebuild","failing_lineage","failing_adversarial")) {
    bash ("examples/{0}/verify.sh" -f $dir)
}

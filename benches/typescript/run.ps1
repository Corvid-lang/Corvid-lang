param(
    [string]$Fixture = "tool_loop",
    [int]$Trials = 3,
    [string]$Output = ""
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)

if (-not $Output) {
    $Output = Join-Path $PSScriptRoot "results\$Fixture.jsonl"
}

Push-Location $PSScriptRoot
try {
    if (-not (Test-Path node_modules)) {
        npm install
    }
    New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Output) | Out-Null
    npx tsx runner.ts (Join-Path $repo "benchmarks\cases\$Fixture.json") $Trials $Output
}
finally {
    Pop-Location
}

param(
    [string]$Fixture = "tool_loop",
    [int]$Trials = 3,
    [string]$Output = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
$repo = Split-Path -Parent $root

if (-not $Output) {
    $Output = Join-Path $PSScriptRoot "results\$Fixture.jsonl"
}

$fixturePath = Join-Path $repo "benchmarks\cases\$Fixture.json"
$runnerManifest = Join-Path $PSScriptRoot "runner\Cargo.toml"

New-Item -ItemType Directory -Force -Path (Split-Path -Parent $Output) | Out-Null

cargo run --manifest-path $runnerManifest -- $fixturePath $Trials $Output

$ErrorActionPreference = "Stop"

$Root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$Source = Join-Path $PSScriptRoot "src\refund_gate.cor"
$OutDir = Join-Path $PSScriptRoot "target\wasm"

Push-Location $Root
try {
    cargo run -q -p corvid-cli -- build $Source --target=wasm
} finally {
    Pop-Location
}

$Required = @(
    "refund_gate.wasm",
    "refund_gate.js",
    "refund_gate.d.ts",
    "refund_gate.corvid-wasm.json"
)

foreach ($Name in $Required) {
    $Path = Join-Path $OutDir $Name
    if (-not (Test-Path $Path)) {
        throw "missing WASM demo artifact: $Path"
    }
}

$Loader = Get-Content (Join-Path $OutDir "refund_gate.js") -Raw
if ($Loader -notmatch "kind: 'approval_decision'") {
    throw "generated loader does not record approval decisions"
}
if ($Loader -notmatch "kind: 'run_completed'") {
    throw "generated loader does not record run completion"
}

$Types = Get-Content (Join-Path $OutDir "refund_gate.d.ts") -Raw
if ($Types -notmatch "CorvidWasmHost") {
    throw "generated TypeScript host interface missing"
}
if ($Types -notmatch "review_refund\(amount: bigint\): bigint") {
    throw "generated TypeScript agent signature missing"
}

$Demo = Get-Content (Join-Path $PSScriptRoot "web\demo.js") -Raw
if ($Demo -notmatch "\.\./target/wasm/refund_gate\.js") {
    throw "browser demo does not import the generated loader"
}

Write-Host "wasm browser demo OK"

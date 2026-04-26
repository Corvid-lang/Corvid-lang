$ErrorActionPreference = "Stop"

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    Write-Error "cargo is required. Install Rust from https://rustup.rs first."
}

Write-Host "Installing corvid-cli with cargo..."
cargo install --path crates/corvid-cli --locked

if (Get-Command rustup -ErrorAction SilentlyContinue) {
    try {
        rustup target add wasm32-unknown-unknown | Out-Null
    } catch {
    }
}

Write-Host ""
Write-Host "Installed. Run:"
Write-Host "  corvid doctor"
Write-Host "  corvid tour --list"

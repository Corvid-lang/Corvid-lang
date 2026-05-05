# Corvid installer for Windows.
#
# One-line invocation:
#   irm https://raw.githubusercontent.com/Micrurus-Ai/Corvid-lang/main/install/install.ps1 | iex
#
# Env var overrides:
#   $env:CORVID_REPO     = "Micrurus-Ai/Corvid-lang"   # source repo
#   $env:CORVID_VERSION  = "latest" | "v0.1.0"         # release tag
#   $env:CORVID_HOME     = "$env:USERPROFILE\.corvid"  # install root

$ErrorActionPreference = 'Stop'

if ($PSVersionTable.PSVersion.Major -lt 5) {
    Write-Error "PowerShell 5.1 or later required. Current: $($PSVersionTable.PSVersion)"
}

# --- config ---------------------------------------------------------------
$Repo    = if ($env:CORVID_REPO)    { $env:CORVID_REPO }    else { 'Micrurus-Ai/Corvid-lang' }
$Version = if ($env:CORVID_VERSION) { $env:CORVID_VERSION } else { 'latest' }
$Root    = if ($env:CORVID_HOME)    { $env:CORVID_HOME }    else { Join-Path $env:USERPROFILE '.corvid' }

# --- helpers --------------------------------------------------------------
function Step($m) { Write-Host "==> $m" -ForegroundColor Cyan }
function Ok($m)   { Write-Host "    $m" -ForegroundColor Green }
function Warn2($m){ Write-Host "    $m" -ForegroundColor Yellow }

function Have($name) { [bool](Get-Command $name -ErrorAction SilentlyContinue) }

# --- target detection -----------------------------------------------------
$arch = switch ($env:PROCESSOR_ARCHITECTURE) {
    'AMD64' { 'x86_64' }
    'ARM64' { 'aarch64' }
    default { throw "Unsupported architecture: $env:PROCESSOR_ARCHITECTURE" }
}
$target    = "$arch-pc-windows-msvc"
$assetName = "corvid-$target.zip"

if ($Version -eq 'latest') {
    $url = "https://github.com/$Repo/releases/latest/download/$assetName"
} else {
    $url = "https://github.com/$Repo/releases/download/$Version/$assetName"
}

# --- prepare install root -------------------------------------------------
if (Test-Path $Root) {
    Step "Removing previous install at $Root"
    Remove-Item -Recurse -Force $Root
}
New-Item -ItemType Directory -Force -Path $Root | Out-Null

$tmp = Join-Path $env:TEMP ("corvid-install-" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

$installed = $false

# --- fast path: prebuilt archive -----------------------------------------
try {
    Step "Downloading $assetName"
    $zip = Join-Path $tmp $assetName
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing -ErrorAction Stop

    Step "Extracting to $Root"
    Expand-Archive -Path $zip -DestinationPath $tmp -Force

    $extracted = Join-Path $tmp "corvid-$target"
    $payload = if (Test-Path $extracted) { $extracted } else { $tmp }
    Get-ChildItem -Path $payload -Force | Where-Object { $_.FullName -ne $zip } | ForEach-Object {
        Move-Item -Path $_.FullName -Destination $Root -Force
    }

    $installed = $true
    Ok "Installed prebuilt binary"
} catch {
    Warn2 "Prebuilt archive unavailable: $($_.Exception.Message)"
    Warn2 "Falling back to cargo source build (this can take 5-15 minutes)..."
}

# --- fallback path: build from source ------------------------------------
if (-not $installed) {
    if (-not (Have 'cargo')) {
        Step "Installing Rust toolchain..."
        $cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
        if (Have 'winget') {
            winget install --id Rustlang.Rustup --silent --accept-source-agreements --accept-package-agreements
        } else {
            $rustupInit = Join-Path $tmp 'rustup-init.exe'
            Invoke-WebRequest -Uri 'https://win.rustup.rs/x86_64' -OutFile $rustupInit -UseBasicParsing
            & $rustupInit -y --default-toolchain stable --profile minimal
        }
        if (Test-Path $cargoBin) { $env:PATH = "$cargoBin;$env:PATH" }
        if (-not (Have 'cargo')) { throw "Rust install completed but cargo is not on PATH. Open a new shell and re-run." }
    }

    if (-not (Have 'git')) {
        throw "git is required for the source build fallback. Install Git for Windows from https://git-scm.com/download/win and re-run."
    }

    $src = Join-Path $Root 'src'
    Step "Cloning $Repo to $src"
    git clone --depth 1 "https://github.com/$Repo.git" $src

    Step "Building corvid (this is the slow part; first build downloads many crates)"
    Push-Location $src
    try {
        cargo install --path crates/corvid-cli --locked --root $Root
    } finally {
        Pop-Location
    }

    $stdSrc = Join-Path $src 'std'
    if (Test-Path $stdSrc) {
        Copy-Item -Recurse -Force $stdSrc (Join-Path $Root 'std')
    }
    Ok "Built from source"
}

# --- PATH wiring ----------------------------------------------------------
$bin = Join-Path $Root 'bin'
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
$onPath = ($userPath -split ';' | Where-Object { $_ -and ($_.TrimEnd('\') -ieq $bin.TrimEnd('\')) })
if (-not $onPath) {
    Step "Adding $bin to user PATH"
    $newPath = if ([string]::IsNullOrEmpty($userPath)) { $bin } else { "$userPath;$bin" }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
} else {
    Ok "$bin already on user PATH"
}
$env:PATH = "$bin;$env:PATH"

[Environment]::SetEnvironmentVariable('CORVID_HOME', $Root, 'User')
$env:CORVID_HOME = $Root

# --- verify ---------------------------------------------------------------
$exe = Join-Path $bin 'corvid.exe'
if (Test-Path $exe) {
    Step "corvid doctor"
    try { & $exe doctor } catch { Warn2 "doctor reported issues; see output above" }
} else {
    Warn2 "corvid.exe not found at $exe -- install may be incomplete"
}

Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "Corvid installed at $Root" -ForegroundColor Green
Write-Host "Open a NEW terminal, then try:" -ForegroundColor Green
Write-Host "  corvid tour --list"
Write-Host "  corvid check examples\refund_bot_demo\refund.cor"

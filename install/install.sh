#!/usr/bin/env sh
# Corvid installer for macOS and Linux.
#
# One-line invocation:
#   curl -fsSL https://raw.githubusercontent.com/Micrurus-Ai/Corvid-lang/main/install/install.sh | sh
#
# Env var overrides:
#   CORVID_REPO     (default: Micrurus-Ai/Corvid-lang)
#   CORVID_VERSION  (default: latest)
#   CORVID_HOME     (default: $HOME/.corvid)

set -eu

REPO="${CORVID_REPO:-Micrurus-Ai/Corvid-lang}"
VERSION="${CORVID_VERSION:-latest}"
ROOT="${CORVID_HOME:-$HOME/.corvid}"

# --- helpers --------------------------------------------------------------
step()  { printf '\033[36m==> %s\033[0m\n' "$*"; }
ok()    { printf '\033[32m    %s\033[0m\n' "$*"; }
warn()  { printf '\033[33m    %s\033[0m\n' "$*"; }
die()   { printf '\033[31merror: %s\033[0m\n' "$*" >&2; exit 1; }
have()  { command -v "$1" >/dev/null 2>&1; }

dl() {
    if   have curl; then curl -fsSL --proto '=https' --tlsv1.2 -o "$2" "$1"
    elif have wget; then wget -qO "$2" "$1"
    else die "need curl or wget on PATH"
    fi
}

# --- target detection -----------------------------------------------------
os_kernel="$(uname -s | tr '[:upper:]' '[:lower:]')"
case "$os_kernel" in
    linux*)  os_id='unknown-linux-gnu' ;;
    darwin*) os_id='apple-darwin' ;;
    *)       die "unsupported OS: $os_kernel" ;;
esac

case "$(uname -m)" in
    x86_64|amd64)  arch_id='x86_64' ;;
    aarch64|arm64) arch_id='aarch64' ;;
    *)             die "unsupported architecture: $(uname -m)" ;;
esac

target="${arch_id}-${os_id}"
asset="corvid-${target}.tar.gz"

if [ "$VERSION" = "latest" ]; then
    url="https://github.com/${REPO}/releases/latest/download/${asset}"
else
    url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"
fi

# --- prepare install root -------------------------------------------------
if [ -d "$ROOT" ]; then
    step "Removing previous install at $ROOT"
    rm -rf "$ROOT"
fi
mkdir -p "$ROOT"

tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

installed=0

# --- fast path: prebuilt archive -----------------------------------------
step "Downloading $asset"
if dl "$url" "$tmp/$asset" 2>"$tmp/dl.log"; then
    step "Extracting to $ROOT"
    tar -xzf "$tmp/$asset" -C "$tmp"
    payload="$tmp/corvid-${target}"
    [ -d "$payload" ] || payload="$tmp"
    # Move every entry except the archive itself
    for entry in "$payload"/*; do
        [ -e "$entry" ] || continue
        case "$entry" in
            "$tmp/$asset") continue ;;
        esac
        mv "$entry" "$ROOT/"
    done
    installed=1
    ok "Installed prebuilt binary"
else
    warn "Prebuilt archive unavailable for $target"
    warn "Falling back to cargo source build (this can take 5-15 minutes)..."
fi

# --- fallback path: build from source ------------------------------------
if [ "$installed" -eq 0 ]; then
    if ! have cargo; then
        step "Installing Rust toolchain via rustup..."
        dl 'https://sh.rustup.rs' "$tmp/rustup-init.sh"
        sh "$tmp/rustup-init.sh" -y --default-toolchain stable --profile minimal
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
    fi
    have git || die "git is required for the source build fallback. Install git and re-run."

    src="$ROOT/src"
    step "Cloning $REPO to $src"
    git clone --depth 1 "https://github.com/${REPO}.git" "$src"

    step "Building corvid (slow first build; downloads many crates)"
    ( cd "$src" && cargo install --path crates/corvid-cli --locked --root "$ROOT" )

    if [ -d "$src/std" ]; then
        rm -rf "$ROOT/std"
        cp -R "$src/std" "$ROOT/std"
    fi
    ok "Built from source"
fi

# --- PATH wiring ----------------------------------------------------------
bin="$ROOT/bin"
case ":${PATH-}:" in
    *":$bin:"*) ok "$bin already on PATH" ;;
    *)
        step "Adding $bin to PATH in shell rc files"
        added=0
        for rc in "$HOME/.profile" "$HOME/.bashrc" "$HOME/.zshrc"; do
            [ -f "$rc" ] || continue
            if ! grep -q 'CORVID_HOME' "$rc" 2>/dev/null; then
                {
                    printf '\n# corvid\n'
                    printf 'export CORVID_HOME="%s"\n' "$ROOT"
                    printf 'export PATH="$CORVID_HOME/bin:$PATH"\n'
                } >> "$rc"
                ok "  updated $rc"
                added=1
            fi
        done
        if [ "$added" -eq 0 ]; then
            warn "No shell rc file found; add this to your shell profile:"
            warn "  export CORVID_HOME=\"$ROOT\""
            warn "  export PATH=\"\$CORVID_HOME/bin:\$PATH\""
        fi
        ;;
esac

export CORVID_HOME="$ROOT"
export PATH="$bin:$PATH"

# --- verify ---------------------------------------------------------------
if [ -x "$bin/corvid" ]; then
    step "corvid doctor"
    "$bin/corvid" doctor || warn "doctor reported issues; see output above"
else
    warn "corvid not found at $bin/corvid -- install may be incomplete"
fi

printf '\n\033[32mCorvid installed at %s\033[0m\n' "$ROOT"
printf 'Restart your shell (or `source` your rc), then try:\n'
printf '  corvid tour --list\n'
printf '  corvid check examples/refund_bot_demo/refund.cor\n'

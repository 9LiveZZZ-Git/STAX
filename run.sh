#!/usr/bin/env bash
# stax — universal run script for macOS and Linux
# First run: installs Rust and any OS audio dependencies automatically.
set -euo pipefail

RED='\033[0;31m'; GREEN='\033[0;32m'; CYAN='\033[0;36m'; YELLOW='\033[1;33m'; NC='\033[0m'
info()    { printf "${CYAN}[stax]${NC} %s\n" "$*"; }
ok()      { printf "${GREEN}[stax]${NC} %s\n" "$*"; }
warn()    { printf "${YELLOW}[stax]${NC} %s\n" "$*"; }
die()     { printf "${RED}[stax]${NC} %s\n" "$*" >&2; exit 1; }

# ── 1. Rust ──────────────────────────────────────────────────────────────────
if ! command -v cargo &>/dev/null; then
    info "Rust not found — installing via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --no-modify-path
    # Source the env so cargo is on PATH for the rest of this session
    # shellcheck source=/dev/null
    source "${HOME}/.cargo/env"
fi

# After sourcing, still not found → try the well-known path
if ! command -v cargo &>/dev/null; then
    export PATH="${HOME}/.cargo/bin:${PATH}"
fi

command -v cargo &>/dev/null || die "cargo not found even after rustup install. Restart your terminal and re-run."
ok "Rust $(rustc --version | cut -d' ' -f2)"

# ── 2. OS-specific audio dependencies ───────────────────────────────────────
OS="$(uname)"
case "${OS}" in
    Linux)
        info "Linux detected — checking ALSA/JACK audio headers..."

        # pkg-config is the most reliable probe for libasound
        if ! pkg-config --exists alsa 2>/dev/null; then
            info "libasound2-dev not found — installing audio dependencies..."

            if command -v apt-get &>/dev/null; then
                sudo apt-get update -qq
                sudo apt-get install -y -q \
                    libasound2-dev libjack-jackd2-dev pkg-config libxkbcommon-dev \
                    libwayland-dev libglib2.0-dev libfontconfig1-dev
            elif command -v dnf &>/dev/null; then
                sudo dnf install -y \
                    alsa-lib-devel jack-audio-connection-kit-devel pkgconf \
                    libxkbcommon-devel wayland-devel glib2-devel fontconfig-devel
            elif command -v pacman &>/dev/null; then
                sudo pacman -S --noconfirm \
                    alsa-lib jack2 pkg-config libxkbcommon wayland glib2 fontconfig
            elif command -v zypper &>/dev/null; then
                sudo zypper install -y \
                    alsa-devel libjack-devel pkg-config libxkbcommon-devel wayland-devel
            elif command -v apk &>/dev/null; then
                sudo apk add --no-cache \
                    alsa-lib-dev jack-dev pkgconf libxkbcommon-dev wayland-dev fontconfig-dev
            else
                warn "Unknown package manager."
                warn "Install these packages manually: libasound2-dev libjack-jackd2-dev libxkbcommon-dev"
                warn "Then re-run this script."
                exit 1
            fi
        fi
        ok "ALSA audio headers OK"
        ;;

    Darwin)
        # CoreAudio ships with macOS; only the Xcode CLT linker is required.
        if ! xcode-select -p &>/dev/null 2>&1; then
            info "Xcode Command Line Tools not found — installing..."
            xcode-select --install
            info "A dialog will appear to confirm the installation."
            info "Press Enter here once it has finished, then this script will continue."
            read -r
        fi
        ok "macOS CoreAudio OK"
        ;;

    *)
        warn "Unrecognised OS '${OS}'. Proceeding — audio may not work without manual setup."
        ;;
esac

# ── 3. First-run: fetch dependencies (cached after first build) ──────────────
info "Building stax-editor (first run fetches crates; may take 1–3 min)..."
cargo build --bin stax-editor --release

# ── 4. Launch ────────────────────────────────────────────────────────────────
ok "Launching stax editor..."
exec cargo run --bin stax-editor --release

# stax — universal run script for Windows (PowerShell 5.1+)
# First run: installs Rust via rustup-init if not found, then builds and launches.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File run.ps1
#   (or double-click run.bat which calls this script)

$ErrorActionPreference = 'Stop'

function info  ([string]$m) { Write-Host "[stax] $m" -ForegroundColor Cyan    }
function ok    ([string]$m) { Write-Host "[stax] $m" -ForegroundColor Green   }
function warn  ([string]$m) { Write-Host "[stax] $m" -ForegroundColor Yellow  }
function die   ([string]$m) { Write-Host "[stax] $m" -ForegroundColor Red; exit 1 }

# ── 1. Rust / cargo ──────────────────────────────────────────────────────────
$cargo = Get-Command cargo -ErrorAction SilentlyContinue

if (-not $cargo) {
    info "Rust not found — downloading rustup-init.exe..."

    $rustupUrl  = "https://static.rust-lang.org/rustup/dist/x86_64-pc-windows-msvc/rustup-init.exe"
    $rustupExe  = Join-Path $env:TEMP "rustup-init.exe"

    try {
        # Use TLS 1.2+; older PowerShell defaults to TLS 1.0 which many servers reject
        [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
        Invoke-WebRequest -Uri $rustupUrl -OutFile $rustupExe -UseBasicParsing
    } catch {
        die "Failed to download rustup-init.exe: $_"
    }

    info "Running rustup installer (this may open a console window)..."
    $proc = Start-Process -FilePath $rustupExe -ArgumentList "-y", "--no-modify-path" -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        die "rustup-init.exe exited with code $($proc.ExitCode). Try installing Rust manually: https://rustup.rs"
    }

    # Add cargo to PATH for this session
    $cargoBin = Join-Path $env:USERPROFILE ".cargo\bin"
    if ($env:PATH -notlike "*$cargoBin*") {
        $env:PATH = "$cargoBin;$env:PATH"
    }

    $cargo = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cargo) {
        die "cargo still not found after install. Please restart PowerShell and re-run this script."
    }
}

$rustVer = (& rustc --version).Split(' ')[1]
ok "Rust $rustVer"

# ── 2. Windows audio: WASAPI ships with Windows — no extra deps needed ───────
ok "Windows WASAPI audio: built-in, no extra packages required."

# On Windows, egui/winit use Win32 APIs that are always present.
# Visual C++ Redistributable is required; it is installed with the Rust toolchain
# via the MSVC target, which rustup-init sets up automatically.

# ── 3. First-run build ───────────────────────────────────────────────────────
info "Building stax-editor (first run fetches crates; may take 1-3 min)..."
& cargo build --bin stax-editor --release
if ($LASTEXITCODE -ne 0) {
    die "Build failed. Check the output above for errors."
}

# ── 4. Launch ────────────────────────────────────────────────────────────────
ok "Launching stax editor..."
& cargo run --bin stax-editor --release

#!/usr/bin/env bash
# Bootstrap PACK: optional distro packages, toolchain checks, release build + optional UI.
#
#   bash scripts/bootstrap.sh                    # apt deps + release build; UI if dist missing
#   bash scripts/bootstrap.sh --no-apt           # skip apt; verify tools and build only
#   bash scripts/bootstrap.sh --skip-frontend      # backend only (no Node)
#   bash scripts/bootstrap.sh --force-frontend   # rebuild frontend/dist even if present
#   bash scripts/bootstrap.sh --deps-only        # apt + verify cargo; no compile
#
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

NO_APT=0
DEPS_ONLY=0
SKIP_FRONTEND=0
FORCE_FRONTEND=0
DIST_INDEX="$REPO_ROOT/frontend/dist/index.html"

usage() {
  cat <<'EOF'
Usage: bash scripts/bootstrap.sh [OPTIONS]

  --no-apt           Do not run apt-get (install build deps manually; see README).
  --skip-frontend    Build backend only; do not require Node or npm.
  --force-frontend   Run npm ci && npm run build even if frontend/dist exists.
  --deps-only        Install/verify dependencies only; do not compile.
  -h, --help         Show this help.

Environment: run from repository root. Requires bash, curl (for rustup hint only).
EOF
}

for arg in "$@"; do
  case "$arg" in
    --no-apt) NO_APT=1 ;;
    --deps-only) DEPS_ONLY=1 ;;
    --skip-frontend) SKIP_FRONTEND=1 ;;
    --force-frontend) FORCE_FRONTEND=1 ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      echo "error: unknown option: $arg" >&2
      usage >&2
      exit 1
      ;;
  esac
done

if [[ "$SKIP_FRONTEND" -eq 1 && "$FORCE_FRONTEND" -eq 1 ]]; then
  echo "error: --skip-frontend and --force-frontend are mutually exclusive." >&2
  exit 1
fi

want_frontend_build() {
  if [[ "$SKIP_FRONTEND" -eq 1 ]]; then
    return 1
  fi
  if [[ "$FORCE_FRONTEND" -eq 1 ]]; then
    return 0
  fi
  [[ ! -f "$DIST_INDEX" ]]
}

detect_distro() {
  if [[ -f /etc/os-release ]]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    echo "${ID:-unknown}"
  else
    echo "unknown"
  fi
}

install_apt_packages() {
  if [[ "$NO_APT" -eq 1 ]]; then
    echo "Skipping apt (--no-apt)."
    return 0
  fi
  if ! command -v apt-get >/dev/null 2>&1; then
    local id
    id="$(detect_distro)"
    echo "note: apt-get not found (distro: ${id}). Install build deps manually — see README §3." >&2
    return 0
  fi
  echo "Installing Debian/Ubuntu build dependencies (sudo)…"
  sudo apt-get update
  sudo apt-get install -y \
    build-essential \
    pkg-config \
    cmake \
    git \
    curl \
    libpcap-dev \
    libssl-dev \
    iw \
    ca-certificates
  echo "Optional runtime packages (gpsd, bluez): sudo apt install -y gpsd gpsd-clients bluez"
}

require_cargo() {
  if command -v cargo >/dev/null 2>&1; then
    return 0
  fi
  echo "error: cargo not found. Install Rust (stable) with rustup:" >&2
  echo '  curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y' >&2
  echo '  source "$HOME/.cargo/env"' >&2
  exit 1
}

require_node20() {
  if ! command -v node >/dev/null 2>&1; then
    echo "error: node not found. Install Node.js 20+ (see docs/FRONTEND_DEVELOPMENT.md)." >&2
    exit 1
  fi
  local major
  major="$(node -p "parseInt(process.versions.node.split('.')[0], 10)" 2>/dev/null || echo 0)"
  if [[ -z "$major" ]] || [[ "$major" -lt 20 ]]; then
    echo "error: Node.js 20+ required (found: $(node --version 2>/dev/null || echo missing))." >&2
    exit 1
  fi
  if ! command -v npm >/dev/null 2>&1; then
    echo "error: npm not found." >&2
    exit 1
  fi
}

build_backend() {
  echo "Building pack (release)…"
  cargo build --release -p pack
}

build_frontend() {
  echo "Building frontend (npm ci && npm run build)…"
  (
    cd frontend
    npm ci
    npm run build
  )
}

print_finish() {
  local bin="$REPO_ROOT/target/release/pack"
  cat <<EOF

Bootstrap complete.

Run from repository root (creates ./data by default):

  sudo $bin

Or: sudo cargo run -p pack

Dashboard: http://127.0.0.1:8787/  (uses frontend/dist/ when present)

See README for monitor mode, gpsd, BLE, and PACK_DATA.
EOF
}

main() {
  local distro
  distro="$(detect_distro)"
  echo "PACK bootstrap (distro: ${distro})"

  install_apt_packages
  require_cargo

  if want_frontend_build; then
    require_node20
  fi

  if [[ "$DEPS_ONLY" -eq 1 ]]; then
    echo "Dependencies OK (--deps-only; no build)."
    exit 0
  fi

  build_backend

  if want_frontend_build; then
    build_frontend
  elif [[ -f "$DIST_INDEX" ]]; then
    echo "Using committed frontend/dist/ (pass --force-frontend to rebuild)."
  else
    echo "note: frontend/dist/index.html missing; dashboard unavailable until you build the UI." >&2
    echo "      See docs/FRONTEND_DEVELOPMENT.md or run with --force-frontend." >&2
  fi

  print_finish
}

main "$@"

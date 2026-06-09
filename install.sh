#!/bin/sh
set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

info()  { printf "${GREEN}[INFO]${NC}  %s\n" "$1"; }
warn()  { printf "${YELLOW}[WARN]${NC}  %s\n" "$1"; }
error() { printf "${RED}[ERROR]${NC} %s\n" "$1"; exit 1; }

# Detect OS and architecture
detect_os_arch() {
    OS=$(uname -s)
    ARCH=$(uname -m)
    info "Detected: OS=$OS ARCH=$ARCH"

    case "$OS" in
        Darwin)
            case "$ARCH" in
                x86_64)  PLATFORM="macOS Intel (x86_64)" ;;
                arm64)   PLATFORM="macOS Apple Silicon (arm64)" ;;
                *)       error "Unsupported macOS architecture: $ARCH" ;;
            esac
            ;;
        Linux)
            if [ -f /etc/os-release ]; then
                . /etc/os-release
                case "$ID" in
                    ubuntu)
                        MAJOR_VER=$(echo "$VERSION_ID" | cut -d. -f1)
                        [ "$MAJOR_VER" -ge 22 ] || error "Ubuntu 22.04+ required (found $VERSION_ID)"
                        PLATFORM="Ubuntu $VERSION_ID"
                        ;;
                    rocky)
                        MAJOR_VER=$(echo "$VERSION_ID" | cut -d. -f1)
                        [ "$MAJOR_VER" -ge 8 ] || error "Rocky Linux 8+ required (found $VERSION_ID)"
                        PLATFORM="Rocky Linux $VERSION_ID"
                        ;;
                    *)
                        warn "Untested Linux distribution: $ID. Proceeding anyway."
                        PLATFORM="Linux ($ID)"
                        ;;
                esac
            else
                warn "Cannot detect Linux distribution. Proceeding anyway."
                PLATFORM="Linux (unknown)"
            fi
            ;;
        *)
            error "Unsupported OS: $OS"
            ;;
    esac
    info "Platform: $PLATFORM"
}

# Check Docker is installed
check_docker() {
    if command -v docker >/dev/null 2>&1; then
        info "Docker found: $(docker --version)"
    else
        case "$OS" in
            Darwin)
                error "Docker not found. Install Docker Desktop: https://docs.docker.com/desktop/install/mac-install/" ;;
            Linux)
                case "${ID:-unknown}" in
                    ubuntu)
                        error "Docker not found. Install: sudo apt-get update && sudo apt-get install -y docker.io" ;;
                    rocky)
                        error "Docker not found. Install: sudo dnf install -y docker-ce" ;;
                    *)
                        error "Docker not found. Install Docker for your distribution." ;;
                esac
                ;;
        esac
    fi
}

# Install Rust if missing
install_rust() {
    if command -v cargo >/dev/null 2>&1; then
        info "Rust found: $(rustc --version)"
    else
        info "Installing Rust via rustup..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        . "$HOME/.cargo/env"
        info "Rust installed: $(rustc --version)"
    fi
}

# Build
build() {
    info "Building release binary..."
    cargo build --release
    info "Build complete."
}

# Build Docker image
build_image() {
    info "Building Docker image emr:latest..."
    docker build -t emr:latest .
    info "Docker image built."
}

# Install binary
install_binary() {
    info "Installing emr to /usr/local/bin..."
    sudo cp target/release/emr /usr/local/bin/emr
    sudo chmod +x /usr/local/bin/emr
    info "Installed."
}

# Verify
verify() {
    info "Verifying installation..."
    emr --version
    docker images emr --format "{{.Repository}}:{{.Tag}} ({{.Size}})"
    info "Installation complete!"
}

# Main
detect_os_arch
check_docker
install_rust
build
build_image
install_binary
verify

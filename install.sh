#!/bin/bash
# Horcrux Installation Script
#
# This script installs Horcrux on a Linux system with optional components.
#
# Usage:
#   ./install.sh [OPTIONS]
#
# Options:
#   --prefix PREFIX     Installation prefix (default: /usr/local)
#   --with-ui           Install Web UI components
#   --with-systemd      Install systemd service files
#   --with-openrc       Install OpenRC service files
#   --dev               Development mode (debug builds)
#   --help              Show this help message

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Default options
PREFIX="/usr/local"
INSTALL_UI=false
INSTALL_SYSTEMD=false
INSTALL_OPENRC=false
DEV_MODE=false
BUILD_MODE="release"

# Parse command line arguments
while [[ $# -gt 0 ]]; do
    case $1 in
        --prefix)
            PREFIX="$2"
            shift 2
            ;;
        --with-ui)
            INSTALL_UI=true
            shift
            ;;
        --with-systemd)
            INSTALL_SYSTEMD=true
            shift
            ;;
        --with-openrc)
            INSTALL_OPENRC=true
            shift
            ;;
        --dev)
            DEV_MODE=true
            BUILD_MODE="debug"
            shift
            ;;
        --help)
            grep "^#" "$0" | sed 's/^# //'
            exit 0
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            echo "Use --help for usage information"
            exit 1
            ;;
    esac
done

# Helper functions
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warning() {
    echo -e "${YELLOW}[WARNING]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

check_command() {
    if ! command -v "$1" &> /dev/null; then
        log_error "$1 is not installed"
        return 1
    fi
    log_success "$1 is installed"
    return 0
}

# Print banner
echo -e "${BLUE}"
cat << "EOF"
    __  __
   / / / /___  _____________  ____  __  __
  / /_/ / __ \/ ___/ ___/ / / / / |/_/
 / __  / /_/ / /  / /__/ /_/ />  <
/_/ /_/\____/_/   \___/\__,_/_/|_|

Proxmox VE Alternative for Gentoo Linux
EOF
echo -e "${NC}"

log_info "Starting Horcrux installation..."
log_info "Installation prefix: $PREFIX"
log_info "Build mode: $BUILD_MODE"

# Check prerequisites
log_info "Checking prerequisites..."

if ! check_command "cargo"; then
    log_error "Rust is not installed"
    log_info "Install Rust from: https://rustup.rs/"
    exit 1
fi

# Check Rust version
RUST_VERSION=$(rustc --version | awk '{print $2}')
log_info "Rust version: $RUST_VERSION"

# Check for required system commands
log_info "Checking for optional system components..."

QEMU_AVAILABLE=false
LXC_AVAILABLE=false
DOCKER_AVAILABLE=false

if check_command "qemu-system-x86_64"; then
    QEMU_AVAILABLE=true
fi

if check_command "lxc-info"; then
    LXC_AVAILABLE=true
fi

if check_command "docker"; then
    DOCKER_AVAILABLE=true
fi

# Build Horcrux
log_info "Building Horcrux..."

if [ "$BUILD_MODE" = "release" ]; then
    log_info "Building release version (optimized)..."
    cargo build --release --bin horcrux-api --bin horcrux-cli
    BIN_DIR="target/release"
else
    log_info "Building debug version (faster compilation)..."
    cargo build --bin horcrux-api --bin horcrux-cli
    BIN_DIR="target/debug"
fi

if [ $? -ne 0 ]; then
    log_error "Build failed"
    exit 1
fi

log_success "Build completed successfully"

# Build Web UI if requested
if [ "$INSTALL_UI" = true ]; then
    log_info "Building Web UI..."

    if ! check_command "trunk"; then
        log_warning "trunk not found, installing..."
        cargo install trunk
    fi

    # Add WASM target if not present
    log_info "Adding wasm32-unknown-unknown target..."
    rustup target add wasm32-unknown-unknown

    cd horcrux-api/horcrux-ui
    if [ "$BUILD_MODE" = "release" ]; then
        trunk build --release
    else
        trunk build
    fi
    cd ../..

    log_success "Web UI built successfully"
fi

# Create installation directories
log_info "Creating installation directories..."

sudo mkdir -p "$PREFIX/bin"
sudo mkdir -p "$PREFIX/share/horcrux"
sudo mkdir -p "/etc/horcrux"
sudo mkdir -p "/var/lib/horcrux"
sudo mkdir -p "/var/log/horcrux"

# Install binaries
log_info "Installing binaries..."

sudo install -m 755 "$BIN_DIR/horcrux-api" "$PREFIX/bin/"
sudo install -m 755 "$BIN_DIR/horcrux-cli" "$PREFIX/bin/"

log_success "Binaries installed to $PREFIX/bin"

# Install Web UI files if built
if [ "$INSTALL_UI" = true ]; then
    log_info "Installing Web UI files..."
    sudo cp -r horcrux-api/horcrux-ui/dist/* "$PREFIX/share/horcrux/"
    log_success "Web UI installed to $PREFIX/share/horcrux"
fi

# Install configuration file
if [ ! -f "/etc/horcrux/config.toml" ]; then
    log_info "Installing default configuration..."
    sudo install -m 644 deploy/config.toml.example "/etc/horcrux/config.toml"
    log_success "Configuration installed to /etc/horcrux/config.toml"
else
    log_warning "Configuration file already exists, skipping"
fi

# Install systemd service files
if [ "$INSTALL_SYSTEMD" = true ]; then
    log_info "Installing systemd service files..."

    if [ -d "/etc/systemd/system" ]; then
        sudo install -m 644 deploy/systemd/horcrux.service /etc/systemd/system/
        sudo install -m 644 deploy/systemd/horcrux-monitoring.service /etc/systemd/system/
        sudo systemctl daemon-reload
        log_success "systemd service files installed"
        log_info "Enable and start with: sudo systemctl enable --now horcrux"
    else
        log_warning "systemd not found, skipping service installation"
    fi
fi

# Install OpenRC service files
if [ "$INSTALL_OPENRC" = true ]; then
    log_info "Installing OpenRC service files..."

    if [ -d "/etc/init.d" ]; then
        sudo install -m 755 deploy/openrc/horcrux /etc/init.d/
        sudo install -m 755 deploy/openrc/horcrux-monitoring /etc/init.d/
        log_success "OpenRC service files installed"
        log_info "Enable and start with: sudo rc-update add horcrux default && sudo rc-service horcrux start"
    else
        log_warning "OpenRC not found, skipping service installation"
    fi
fi

# Set up database
log_info "Initializing database..."
sudo touch /var/lib/horcrux/horcrux.db
sudo chmod 644 /var/lib/horcrux/horcrux.db

# Print summary
echo ""
log_success "========================================="
log_success "Horcrux Installation Complete!"
log_success "========================================="
echo ""
log_info "Installed components:"
log_info "  - horcrux-api:     $PREFIX/bin/horcrux-api"
log_info "  - horcrux-cli:     $PREFIX/bin/horcrux-cli"
log_info "  - Configuration:   /etc/horcrux/config.toml"
log_info "  - Data directory:  /var/lib/horcrux"
log_info "  - Log directory:   /var/log/horcrux"

if [ "$INSTALL_UI" = true ]; then
    log_info "  - Web UI:          $PREFIX/share/horcrux"
fi

echo ""
log_info "Available hypervisors:"
[ "$QEMU_AVAILABLE" = true ] && log_success "  ✓ QEMU/KVM" || log_warning "  ✗ QEMU/KVM (not installed)"
[ "$LXC_AVAILABLE" = true ] && log_success "  ✓ LXC" || log_warning "  ✗ LXC (not installed)"
[ "$DOCKER_AVAILABLE" = true ] && log_success "  ✓ Docker" || log_warning "  ✗ Docker (not installed)"

echo ""
log_info "Next steps:"
echo ""
echo "  1. Edit configuration:"
echo "     sudo \$EDITOR /etc/horcrux/config.toml"
echo ""
echo "  2. Start the API server:"

if [ "$INSTALL_SYSTEMD" = true ]; then
    echo "     sudo systemctl enable --now horcrux"
elif [ "$INSTALL_OPENRC" = true ]; then
    echo "     sudo rc-update add horcrux default"
    echo "     sudo rc-service horcrux start"
else
    echo "     horcrux-api"
fi

echo ""
echo "  3. Access the Web UI:"
echo "     http://localhost:8006"
echo ""
echo "  4. Use the CLI:"
echo "     horcrux-cli vm list"
echo "     horcrux-cli --help"
echo ""
log_info "For more information, see:"
log_info "  - Documentation:  ./docs/"
log_info "  - README:         ./README.md"
log_info "  - Contributing:   ./CONTRIBUTING.md"
echo ""
log_success "Enjoy using Horcrux! 🎉"

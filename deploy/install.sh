#!/bin/bash
# Horcrux Installation Script for Gentoo Linux
# This script installs Horcrux virtualization management platform

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Configuration
INSTALL_DIR="/opt/horcrux"
CONFIG_DIR="/etc/horcrux"
DATA_DIR="/var/lib/horcrux"
LOG_DIR="/var/log/horcrux"
USER="horcrux"
GROUP="horcrux"

# Print colored messages
log_info() {
    echo -e "${BLUE}[INFO]${NC} $1"
}

log_success() {
    echo -e "${GREEN}[SUCCESS]${NC} $1"
}

log_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

log_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Check if running as root
check_root() {
    if [[ $EUID -ne 0 ]]; then
        log_error "This script must be run as root"
        exit 1
    fi
}

# Check system requirements
check_requirements() {
    log_info "Checking system requirements..."

    # Check if running on Gentoo
    if [[ ! -f /etc/gentoo-release ]]; then
        log_warn "This script is designed for Gentoo Linux"
        read -p "Continue anyway? (y/n) " -n 1 -r
        echo
        if [[ ! $REPLY =~ ^[Yy]$ ]]; then
            exit 1
        fi
    fi

    # Check for required commands
    local required_cmds=("cargo" "rustc" "systemctl" "sqlite3")
    for cmd in "${required_cmds[@]}"; do
        if ! command -v $cmd &> /dev/null; then
            log_error "Required command not found: $cmd"
            log_info "Please install: $cmd"
            exit 1
        fi
    done

    # Check Rust version
    local rust_version=$(rustc --version | awk '{print $2}')
    log_info "Rust version: $rust_version"

    # Check for KVM support
    if [[ ! -e /dev/kvm ]]; then
        log_warn "KVM not available. VM functionality may be limited."
    fi

    log_success "System requirements check passed"
}

# Create user and group
create_user() {
    log_info "Creating horcrux user and group..."

    if ! getent group $GROUP > /dev/null 2>&1; then
        groupadd -r $GROUP
        log_success "Created group: $GROUP"
    else
        log_info "Group already exists: $GROUP"
    fi

    if ! id -u $USER > /dev/null 2>&1; then
        useradd -r -g $GROUP -d $DATA_DIR -s /bin/bash -c "Horcrux Service User" $USER
        log_success "Created user: $USER"
    else
        log_info "User already exists: $USER"
    fi

    # Add horcrux user to kvm and libvirt groups (if they exist)
    for group in kvm libvirt docker; do
        if getent group $group > /dev/null 2>&1; then
            usermod -a -G $group $USER
            log_info "Added $USER to group: $group"
        fi
    done
}

# Create directories
create_directories() {
    log_info "Creating directories..."

    local dirs=(
        "$INSTALL_DIR"
        "$CONFIG_DIR"
        "$DATA_DIR"
        "$DATA_DIR/vms"
        "$DATA_DIR/snapshots"
        "$DATA_DIR/backups"
        "$DATA_DIR/templates"
        "$DATA_DIR/cloudinit"
        "$LOG_DIR"
    )

    for dir in "${dirs[@]}"; do
        if [[ ! -d "$dir" ]]; then
            mkdir -p "$dir"
            log_success "Created directory: $dir"
        fi
    done

    # Set ownership
    chown -R $USER:$GROUP $DATA_DIR
    chown -R $USER:$GROUP $LOG_DIR

    log_success "Directory structure created"
}

# Build Horcrux
build_horcrux() {
    log_info "Building Horcrux from source..."

    # Get script directory
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

    cd "$SCRIPT_DIR"

    # Build release binaries
    log_info "Compiling API server..."
    cargo build --release -p horcrux-api

    log_info "Compiling CLI tool..."
    cargo build --release -p horcrux-cli

    log_info "Building Web UI..."
    if command -v trunk &> /dev/null; then
        cd horcrux-api/horcrux-ui
        trunk build --release
        cd ../..
    else
        log_warn "trunk not found. Web UI will not be built."
        log_info "Install trunk with: cargo install trunk"
    fi

    log_success "Build completed"
}

# Install binaries
install_binaries() {
    log_info "Installing binaries..."

    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

    # Install API server
    if [[ -f "$SCRIPT_DIR/target/release/horcrux-api" ]]; then
        cp "$SCRIPT_DIR/target/release/horcrux-api" /usr/bin/
        chmod 755 /usr/bin/horcrux-api
        log_success "Installed: /usr/bin/horcrux-api"
    fi

    # Install CLI tool
    if [[ -f "$SCRIPT_DIR/target/release/horcrux" ]]; then
        cp "$SCRIPT_DIR/target/release/horcrux" /usr/bin/
        chmod 755 /usr/bin/horcrux
        log_success "Installed: /usr/bin/horcrux"
    fi

    # Install Web UI
    if [[ -d "$SCRIPT_DIR/horcrux-api/horcrux-ui/dist" ]]; then
        cp -r "$SCRIPT_DIR/horcrux-api/horcrux-ui/dist" "$INSTALL_DIR/"
        log_success "Installed: $INSTALL_DIR/dist"
    fi
}

# Install configuration
install_config() {
    log_info "Installing configuration files..."

    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

    # Install main config
    if [[ ! -f "$CONFIG_DIR/horcrux.toml" ]]; then
        cp "$SCRIPT_DIR/deploy/config/horcrux.toml" "$CONFIG_DIR/"
        chmod 644 "$CONFIG_DIR/horcrux.toml"
        log_success "Installed: $CONFIG_DIR/horcrux.toml"

        # Generate random JWT secret
        JWT_SECRET=$(openssl rand -base64 32)
        sed -i "s/CHANGE_ME_TO_A_RANDOM_SECRET_KEY/$JWT_SECRET/" "$CONFIG_DIR/horcrux.toml"
        log_success "Generated JWT secret"
    else
        log_info "Configuration already exists: $CONFIG_DIR/horcrux.toml"
    fi
}

# Initialize database
init_database() {
    log_info "Initializing database..."

    DB_PATH="$DATA_DIR/horcrux.db"

    if [[ ! -f "$DB_PATH" ]]; then
        # Create empty database
        sudo -u $USER sqlite3 "$DB_PATH" "VACUUM;"

        # Set permissions
        chmod 640 "$DB_PATH"
        chown $USER:$GROUP "$DB_PATH"

        log_success "Database initialized: $DB_PATH"
        log_info "Database will be migrated on first startup"
    else
        log_info "Database already exists: $DB_PATH"
    fi
}

# Install systemd services
install_services() {
    log_info "Installing systemd services..."

    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && cd .. && pwd)"

    # Copy service files
    cp "$SCRIPT_DIR/deploy/systemd/horcrux-api.service" /etc/systemd/system/
    cp "$SCRIPT_DIR/deploy/systemd/horcrux-metrics.service" /etc/systemd/system/

    # Reload systemd
    systemctl daemon-reload

    log_success "Systemd services installed"
}

# Enable and start services
enable_services() {
    log_info "Enabling and starting services..."

    systemctl enable horcrux-api.service
    systemctl enable horcrux-metrics.service

    log_success "Services enabled"

    read -p "Start services now? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        systemctl start horcrux-api.service
        systemctl start horcrux-metrics.service

        sleep 2

        if systemctl is-active --quiet horcrux-api.service; then
            log_success "Horcrux API server is running"
        else
            log_error "Failed to start Horcrux API server"
            log_info "Check logs with: journalctl -u horcrux-api.service -f"
        fi
    fi
}

# Generate shell completions
generate_completions() {
    log_info "Generating shell completions..."

    if command -v horcrux &> /dev/null; then
        # Bash
        horcrux completions bash > /etc/bash_completion.d/horcrux
        log_success "Generated bash completion"

        # Zsh (if zsh is installed)
        if command -v zsh &> /dev/null; then
            mkdir -p /usr/share/zsh/site-functions
            horcrux completions zsh > /usr/share/zsh/site-functions/_horcrux
            log_success "Generated zsh completion"
        fi
    fi
}

# Print post-install information
print_info() {
    echo
    echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
    echo -e "${GREEN}  Horcrux Installation Complete!${NC}"
    echo -e "${GREEN}═══════════════════════════════════════════════════${NC}"
    echo
    echo -e "${BLUE}Web UI:${NC}      http://localhost:8006"
    echo -e "${BLUE}API Docs:${NC}    http://localhost:8006/api/docs"
    echo -e "${BLUE}Metrics:${NC}     http://localhost:9090/metrics"
    echo
    echo -e "${BLUE}Configuration:${NC} $CONFIG_DIR/horcrux.toml"
    echo -e "${BLUE}Data Dir:${NC}      $DATA_DIR"
    echo -e "${BLUE}Logs:${NC}          $LOG_DIR"
    echo
    echo -e "${YELLOW}Next Steps:${NC}"
    echo "  1. Review configuration: $CONFIG_DIR/horcrux.toml"
    echo "  2. Create admin user: horcrux auth register"
    echo "  3. Check service status: systemctl status horcrux-api"
    echo "  4. View logs: journalctl -u horcrux-api -f"
    echo
    echo -e "${YELLOW}Documentation:${NC}"
    echo "  - README.md"
    echo "  - docs/CLI.md"
    echo "  - docs/API_DOCS.md"
    echo "  - docs/NEW_FEATURES.md"
    echo "  - docs/REALTIME_FEATURES.md"
    echo
    echo -e "${GREEN}Happy virtualizing! 🚀${NC}"
    echo
}

# Main installation flow
main() {
    echo
    echo "═════════════════════════════════════════════════"
    echo "  Horcrux Installation Script for Gentoo Linux"
    echo "═════════════════════════════════════════════════"
    echo

    check_root
    check_requirements
    create_user
    create_directories
    build_horcrux
    install_binaries
    install_config
    init_database
    install_services
    enable_services
    generate_completions
    print_info
}

# Run installation
main "$@"

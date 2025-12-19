# Horcrux ISO Build System Makefile
#
# Usage:
#   make iso-x86_64          # Build minimal x86_64 ISO (binaries only)
#   make iso-aarch64         # Build minimal ARM64 ISO
#   make iso-riscv64         # Build minimal RISC-V ISO
#   make iso-all             # Build all minimal ISOs
#   make gentoo              # Build full Gentoo-based ISO (complete OS)
#   make gentoo-minimal      # Build minimal Gentoo ISO
#   make gentoo-full         # Build full Gentoo ISO with all features
#   make clean               # Clean build artifacts
#
# Variables:
#   BUILD_TYPE=minimal|standard|full (default: standard)
#   VERSION=x.y.z (default: from Cargo.toml)
#   PROFILE=openrc|systemd|hardened (default: openrc)

SHELL := /bin/bash
.ONESHELL:
.SHELLFLAGS := -euo pipefail -c

# Directories
BUILD_DIR := $(shell dirname $(realpath $(lastword $(MAKEFILE_LIST))))
PROJECT_ROOT := $(shell dirname $(BUILD_DIR))
SCRIPTS_DIR := $(BUILD_DIR)/scripts
OUTPUT_DIR := $(BUILD_DIR)/iso
WORK_DIR := $(BUILD_DIR)/work

# Build settings
BUILD_TYPE ?= standard
VERSION ?= $(shell grep '^version = "' $(PROJECT_ROOT)/Cargo.toml | head -1 | sed 's/.*"\(.*\)".*/\1/')
JOBS ?= $(shell nproc)
PROFILE ?= openrc

# Build scripts
BUILD_ISO := $(SCRIPTS_DIR)/build-iso.sh
BUILD_GENTOO_ISO := $(SCRIPTS_DIR)/build-gentoo-iso.sh

# Architectures
ARCHITECTURES := x86_64 aarch64 riscv64

# ISO targets
ISO_X86_64 := $(OUTPUT_DIR)/horcrux-$(VERSION)-x86_64-$(BUILD_TYPE).iso
ISO_AARCH64 := $(OUTPUT_DIR)/horcrux-$(VERSION)-aarch64-$(BUILD_TYPE).iso
ISO_RISCV64 := $(OUTPUT_DIR)/horcrux-$(VERSION)-riscv64-$(BUILD_TYPE).iso

# Colors
BLUE := \033[0;34m
GREEN := \033[0;32m
YELLOW := \033[1;33m
NC := \033[0m

.PHONY: all help iso-x86_64 iso-aarch64 iso-riscv64 iso-all iso-all-parallel clean distclean check-deps
.PHONY: gentoo gentoo-minimal gentoo-standard gentoo-full gentoo-arm64

# Default target
all: iso-all

# Help target
help:
	@echo -e "$(BLUE)Horcrux ISO Build System$(NC)"
	@echo ""
	@echo -e "$(GREEN)Minimal ISOs (binaries only, ~10MB):$(NC)"
	@echo "  iso-x86_64         Build x86_64 minimal ISO"
	@echo "  iso-aarch64        Build ARM64 minimal ISO"
	@echo "  iso-riscv64        Build RISC-V minimal ISO"
	@echo "  iso-all            Build all minimal ISOs"
	@echo ""
	@echo -e "$(GREEN)Full Gentoo ISOs (complete OS, 2-4GB):$(NC)"
	@echo "  gentoo             Build standard Gentoo ISO (amd64)"
	@echo "  gentoo-minimal     Build minimal Gentoo ISO (~1.5GB)"
	@echo "  gentoo-standard    Build standard Gentoo ISO (~2.5GB)"
	@echo "  gentoo-full        Build full Gentoo ISO with all features (~4GB)"
	@echo "  gentoo-arm64       Build Gentoo ISO for ARM64"
	@echo ""
	@echo -e "$(GREEN)Utilities:$(NC)"
	@echo "  clean              Clean build artifacts (keep ISOs)"
	@echo "  distclean          Clean everything including ISOs"
	@echo "  check-deps         Check build dependencies"
	@echo "  info               Show build configuration"
	@echo ""
	@echo "Variables:"
	@echo "  BUILD_TYPE         minimal, standard, full (default: $(BUILD_TYPE))"
	@echo "  PROFILE            openrc, systemd, hardened (default: $(PROFILE))"
	@echo "  VERSION            Version string (default: $(VERSION))"
	@echo "  JOBS               Parallel jobs (default: $(JOBS))"
	@echo ""
	@echo "Examples:"
	@echo "  make iso-x86_64                    # Quick minimal ISO"
	@echo "  make gentoo                        # Full Gentoo OS ISO"
	@echo "  make gentoo-full PROFILE=hardened  # Hardened full build"
	@echo "  sudo make gentoo                   # Must run as root for chroot"

# Check dependencies
check-deps:
	@echo -e "$(BLUE)Checking build dependencies...$(NC)"
	@command -v cargo >/dev/null 2>&1 || { echo "cargo not found"; exit 1; }
	@command -v rustc >/dev/null 2>&1 || { echo "rustc not found"; exit 1; }
	@command -v mksquashfs >/dev/null 2>&1 || { echo "mksquashfs not found (emerge squashfs-tools)"; exit 1; }
	@(command -v xorriso >/dev/null 2>&1 || command -v mkisofs >/dev/null 2>&1 || command -v grub-mkrescue >/dev/null 2>&1) || { echo "xorriso/mkisofs/grub-mkrescue not found (emerge libisoburn or cdrtools)"; exit 1; }
	@command -v cpio >/dev/null 2>&1 || { echo "cpio not found"; exit 1; }
	@echo -e "$(GREEN)All dependencies satisfied$(NC)"

# x86_64 ISO
iso-x86_64: check-deps $(ISO_X86_64)

$(ISO_X86_64):
	@echo -e "$(BLUE)Building x86_64 ISO...$(NC)"
	@mkdir -p $(OUTPUT_DIR)
	$(BUILD_ISO) -a x86_64 -t $(BUILD_TYPE) -o $(OUTPUT_DIR) -j $(JOBS)

# ARM64 ISO
iso-aarch64: check-deps $(ISO_AARCH64)

$(ISO_AARCH64):
	@echo -e "$(BLUE)Building aarch64 ISO...$(NC)"
	@mkdir -p $(OUTPUT_DIR)
	$(BUILD_ISO) -a aarch64 -t $(BUILD_TYPE) -o $(OUTPUT_DIR) -j $(JOBS)

# RISC-V ISO
iso-riscv64: check-deps $(ISO_RISCV64)

$(ISO_RISCV64):
	@echo -e "$(BLUE)Building riscv64 ISO...$(NC)"
	@mkdir -p $(OUTPUT_DIR)
	$(BUILD_ISO) -a riscv64 -t $(BUILD_TYPE) -o $(OUTPUT_DIR) -j $(JOBS)

# Build all ISOs sequentially
iso-all: iso-x86_64 iso-aarch64 iso-riscv64
	@echo -e "$(GREEN)All ISOs built successfully$(NC)"
	@ls -lh $(OUTPUT_DIR)/*.iso

# Build all ISOs in parallel
iso-all-parallel: check-deps
	@echo -e "$(BLUE)Building all ISOs in parallel...$(NC)"
	@mkdir -p $(OUTPUT_DIR)
	@$(MAKE) -j3 iso-x86_64 iso-aarch64 iso-riscv64 --no-print-directory
	@echo -e "$(GREEN)All ISOs built successfully$(NC)"
	@ls -lh $(OUTPUT_DIR)/*.iso

# Clean build artifacts (keep ISOs)
clean:
	@echo -e "$(YELLOW)Cleaning build artifacts...$(NC)"
	rm -rf $(WORK_DIR)
	@echo -e "$(GREEN)Clean complete$(NC)"

# Clean everything including ISOs
distclean: clean
	@echo -e "$(YELLOW)Cleaning ISOs...$(NC)"
	rm -rf $(OUTPUT_DIR)
	@echo -e "$(GREEN)Distclean complete$(NC)"

# Create release tarball with all ISOs
release: iso-all
	@echo -e "$(BLUE)Creating release tarball...$(NC)"
	@mkdir -p $(BUILD_DIR)/releases
	cd $(OUTPUT_DIR) && tar czvf $(BUILD_DIR)/releases/horcrux-$(VERSION)-isos.tar.gz *.iso *.sha256 *.sha512
	@echo -e "$(GREEN)Release: $(BUILD_DIR)/releases/horcrux-$(VERSION)-isos.tar.gz$(NC)"

# Docker-based build (for CI/CD)
docker-build:
	@echo -e "$(BLUE)Building ISOs in Docker...$(NC)"
	docker run --rm -v $(PROJECT_ROOT):/src -w /src/build \
		--platform linux/amd64 \
		ghcr.io/cross-rs/x86_64-unknown-linux-gnu:main \
		make iso-x86_64 BUILD_TYPE=$(BUILD_TYPE)

# Print build info
info:
	@echo "Project Root:  $(PROJECT_ROOT)"
	@echo "Build Dir:     $(BUILD_DIR)"
	@echo "Output Dir:    $(OUTPUT_DIR)"
	@echo "Version:       $(VERSION)"
	@echo "Build Type:    $(BUILD_TYPE)"
	@echo "Profile:       $(PROFILE)"
	@echo "Architectures: $(ARCHITECTURES)"
	@echo "Jobs:          $(JOBS)"

# ============================================================================
# FULL GENTOO ISO TARGETS
# ============================================================================

# Standard Gentoo ISO (recommended)
gentoo: gentoo-standard

# Gentoo ISO variants
gentoo-minimal:
	@echo -e "$(BLUE)Building Gentoo minimal ISO (amd64)...$(NC)"
	@echo -e "$(YELLOW)NOTE: This requires root privileges for chroot operations$(NC)"
	@test $$(id -u) -eq 0 || { echo -e "$(YELLOW)Run with: sudo make gentoo-minimal$(NC)"; exit 1; }
	$(BUILD_GENTOO_ISO) -a amd64 -t minimal -p $(PROFILE) -o $(OUTPUT_DIR) -j $(JOBS)

gentoo-standard:
	@echo -e "$(BLUE)Building Gentoo standard ISO (amd64)...$(NC)"
	@echo -e "$(YELLOW)NOTE: This requires root privileges for chroot operations$(NC)"
	@test $$(id -u) -eq 0 || { echo -e "$(YELLOW)Run with: sudo make gentoo-standard$(NC)"; exit 1; }
	$(BUILD_GENTOO_ISO) -a amd64 -t standard -p $(PROFILE) -o $(OUTPUT_DIR) -j $(JOBS)

gentoo-full:
	@echo -e "$(BLUE)Building Gentoo full ISO (amd64)...$(NC)"
	@echo -e "$(YELLOW)NOTE: This requires root privileges for chroot operations$(NC)"
	@test $$(id -u) -eq 0 || { echo -e "$(YELLOW)Run with: sudo make gentoo-full$(NC)"; exit 1; }
	$(BUILD_GENTOO_ISO) -a amd64 -t full -p $(PROFILE) -o $(OUTPUT_DIR) -j $(JOBS)

gentoo-arm64:
	@echo -e "$(BLUE)Building Gentoo ISO (arm64)...$(NC)"
	@echo -e "$(YELLOW)NOTE: This requires root privileges for chroot operations$(NC)"
	@test $$(id -u) -eq 0 || { echo -e "$(YELLOW)Run with: sudo make gentoo-arm64$(NC)"; exit 1; }
	$(BUILD_GENTOO_ISO) -a arm64 -t $(BUILD_TYPE) -p $(PROFILE) -o $(OUTPUT_DIR) -j $(JOBS)

# Quick kernel skip option (uses binary kernel)
gentoo-quick:
	@echo -e "$(BLUE)Building Gentoo ISO with binary kernel (faster)...$(NC)"
	@test $$(id -u) -eq 0 || { echo -e "$(YELLOW)Run with: sudo make gentoo-quick$(NC)"; exit 1; }
	$(BUILD_GENTOO_ISO) -a amd64 -t standard -p $(PROFILE) -o $(OUTPUT_DIR) -j $(JOBS) --skip-kernel

#!/usr/bin/env bash
# Installation script for HFT Trading System
# Supports: macOS, Ubuntu/Debian, and Windows (via WSL)

set -euo pipefail

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Logging functions
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

# Detect OS
detect_os() {
    if [[ "$OSTYPE" == "linux-gnu"* ]]; then
        if [ -f /etc/os-release ]; then
            . /etc/os-release
            OS=$ID
        else
            OS="linux"
        fi
    elif [[ "$OSTYPE" == "darwin"* ]]; then
        OS="macos"
    elif [[ "$OSTYPE" == "msys" ]] || [[ "$OSTYPE" == "cygwin" ]]; then
        OS="windows"
    else
        OS="unknown"
    fi
    echo "$OS"
}

# Check if command exists
command_exists() {
    command -v "$1" >/dev/null 2>&1
}

# Install Rust
install_rust() {
    if command_exists rustc; then
        local rust_version=$(rustc --version | awk '{print $2}')
        log_info "Rust $rust_version already installed"
        
        # Check if version is >= 1.82
        local major=$(echo $rust_version | cut -d. -f1)
        local minor=$(echo $rust_version | cut -d. -f2)
        
        if [ "$major" -eq 1 ] && [ "$minor" -lt 82 ]; then
            log_warning "Rust 1.82+ required for edition 2024. Updating..."
            rustup update stable
        fi
    else
        log_info "Installing Rust..."
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
        source "$HOME/.cargo/env"
        log_success "Rust installed successfully"
    fi
    
    rustup default stable
}

# Install system dependencies
install_system_deps() {
    local os=$1
    log_info "Installing system dependencies for $os..."
    
    case $os in
        ubuntu|debian)
            sudo apt-get update
            sudo apt-get install -y \
                pkg-config \
                libssl-dev \
                libdbus-1-dev \
                build-essential \
                cmake \
                curl \
                git
            log_success "System dependencies installed"
            ;;
        macos)
            if ! command_exists brew; then
                log_error "Homebrew not found. Please install from https://brew.sh"
                exit 1
            fi
            brew install pkg-config openssl dbus
            log_success "System dependencies installed"
            ;;
        windows)
            log_warning "On Windows, ensure Visual Studio Build Tools 2022 are installed"
            log_info "Download from: https://visualstudio.microsoft.com/downloads/"
            ;;
        *)
            log_warning "Unknown OS. Please install dependencies manually."
            ;;
    esac
}

# Install development tools
install_dev_tools() {
    log_info "Installing development tools..."
    
    # Essential tools
    cargo install cargo-watch || log_warning "cargo-watch installation failed"
    cargo install cargo-audit || log_warning "cargo-audit installation failed"
    cargo install just || log_warning "just installation failed"
    
    log_success "Development tools installed"
}

# Setup project
setup_project() {
    log_info "Setting up project..."
    
    # Create .env file if it doesn't exist
    if [ ! -f .env ]; then
        if [ -f .env.example ]; then
            cp .env.example .env
            log_success "Created .env file from .env.example"
            log_warning "Please edit .env with your API credentials"
        else
            log_warning ".env.example not found"
        fi
    else
        log_info ".env file already exists"
    fi
    
    # Create model directories
    mkdir -p models/crypto models/equity
    log_success "Created model directories"
    
    # Create data directory
    mkdir -p data
    log_success "Created data directory"
}

# Build project
build_project() {
    log_info "Building project (this may take a while)..."
    
    if cargo build --release --all; then
        log_success "Project built successfully"
    else
        log_error "Build failed"
        exit 1
    fi
}

# Run tests
run_tests() {
    log_info "Running tests..."
    
    if cargo test --all; then
        log_success "All tests passed"
    else
        log_warning "Some tests failed"
    fi
}

# Print next steps
print_next_steps() {
    echo ""
    log_success "Installation complete!"
    echo ""
    echo -e "${GREEN}Next steps:${NC}"
    echo "  1. Edit .env file with your API credentials"
    echo "  2. Place ONNX models in models/crypto and models/equity directories"
    echo "  3. Run the engine: cargo run --release -p engine"
    echo "  4. Run the terminal UI: cargo run --release -p terminal"
    echo ""
    echo -e "${BLUE}Useful commands:${NC}"
    echo "  just --list          # Show all available commands (requires 'just')"
    echo "  cargo run -p engine  # Run trading engine"
    echo "  cargo run -p terminal # Run UI terminal"
    echo "  cargo test --all     # Run tests"
    echo "  cargo bench          # Run benchmarks"
    echo ""
    echo -e "${BLUE}Documentation:${NC}"
    echo "  README.md            # Getting started"
    echo "  UPGRADE_GUIDE.md     # Migration notes"
    echo "  cargo doc --open     # API documentation"
    echo ""
}

# Main installation flow
main() {
    echo ""
    echo "================================================"
    echo "  HFT Trading System - Installation Script"
    echo "================================================"
    echo ""
    
    # Detect OS
    OS=$(detect_os)
    log_info "Detected OS: $OS"
    
    # Check prerequisites
    if ! command_exists curl; then
        log_error "curl is required but not installed"
        exit 1
    fi
    
    if ! command_exists git; then
        log_error "git is required but not installed"
        exit 1
    fi
    
    # Install Rust
    install_rust
    
    # Install system dependencies
    install_system_deps "$OS"
    
    # Install development tools (optional)
    read -p "Install development tools (cargo-watch, cargo-audit, just)? [Y/n] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]] || [[ -z $REPLY ]]; then
        install_dev_tools
    fi
    
    # Setup project
    setup_project
    
    # Build project
    read -p "Build project now? [Y/n] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]] || [[ -z $REPLY ]]; then
        build_project
    fi
    
    # Run tests
    read -p "Run tests? [y/N] " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        run_tests
    fi
    
    # Print next steps
    print_next_steps
}

# Run main
main "$@"

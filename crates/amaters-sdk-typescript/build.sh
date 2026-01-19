#!/bin/bash
# Build script for AmateRS TypeScript SDK
#
# This script builds the WASM module and TypeScript bindings
# for both web and Node.js targets.

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$SCRIPT_DIR"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

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

# Check for required tools
check_dependencies() {
    log_info "Checking dependencies..."

    if ! command -v wasm-pack &> /dev/null; then
        log_error "wasm-pack is not installed. Install with: cargo install wasm-pack"
        exit 1
    fi

    if ! command -v npm &> /dev/null; then
        log_warn "npm is not installed. TypeScript build will be skipped."
    fi

    log_success "All required dependencies found"
}

# Clean previous builds
clean() {
    log_info "Cleaning previous builds..."
    rm -rf pkg dist
    log_success "Clean complete"
}

# Build WASM for web target
build_wasm_web() {
    log_info "Building WASM for web target..."
    wasm-pack build --target web --out-dir pkg/web --out-name amaters_sdk
    log_success "Web WASM build complete"
}

# Build WASM for Node.js target
build_wasm_node() {
    log_info "Building WASM for Node.js target..."
    wasm-pack build --target nodejs --out-dir pkg/node --out-name amaters_sdk
    log_success "Node.js WASM build complete"
}

# Build WASM for bundler target (webpack, rollup, etc.)
build_wasm_bundler() {
    log_info "Building WASM for bundler target..."
    wasm-pack build --target bundler --out-dir pkg/bundler --out-name amaters_sdk
    log_success "Bundler WASM build complete"
}

# Build TypeScript wrapper
build_typescript() {
    if ! command -v npm &> /dev/null; then
        log_warn "Skipping TypeScript build (npm not found)"
        return
    fi

    log_info "Installing npm dependencies..."
    npm install

    log_info "Building TypeScript..."
    npm run build:js

    log_success "TypeScript build complete"
}

# Run tests
run_tests() {
    log_info "Running WASM tests..."
    wasm-pack test --headless --chrome
    log_success "Tests complete"
}

# Build optimized release
build_release() {
    log_info "Building optimized release..."
    wasm-pack build --target web --out-dir pkg/web --out-name amaters_sdk --release
    wasm-pack build --target nodejs --out-dir pkg/node --out-name amaters_sdk --release
    wasm-pack build --target bundler --out-dir pkg/bundler --out-name amaters_sdk --release
    log_success "Release build complete"
}

# Display help
show_help() {
    echo "AmateRS TypeScript SDK Build Script"
    echo ""
    echo "Usage: $0 [command]"
    echo ""
    echo "Commands:"
    echo "  all        Build everything (default)"
    echo "  clean      Clean build artifacts"
    echo "  web        Build WASM for web target"
    echo "  node       Build WASM for Node.js target"
    echo "  bundler    Build WASM for bundler target"
    echo "  ts         Build TypeScript wrapper"
    echo "  test       Run tests"
    echo "  release    Build optimized release"
    echo "  help       Show this help message"
    echo ""
    echo "Examples:"
    echo "  $0          # Build everything"
    echo "  $0 clean    # Clean build artifacts"
    echo "  $0 release  # Build optimized release"
}

# Main entry point
main() {
    local command="${1:-all}"

    check_dependencies

    case "$command" in
        all)
            clean
            build_wasm_web
            build_wasm_node
            build_wasm_bundler
            build_typescript
            log_success "All builds complete!"
            ;;
        clean)
            clean
            ;;
        web)
            build_wasm_web
            ;;
        node)
            build_wasm_node
            ;;
        bundler)
            build_wasm_bundler
            ;;
        ts)
            build_typescript
            ;;
        test)
            run_tests
            ;;
        release)
            clean
            build_release
            build_typescript
            log_success "Release build complete!"
            ;;
        help|--help|-h)
            show_help
            ;;
        *)
            log_error "Unknown command: $command"
            show_help
            exit 1
            ;;
    esac
}

main "$@"

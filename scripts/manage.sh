#!/usr/bin/env bash
# Management script for rate-mirrors repository

set -euo pipefail

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Script directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CARGO_TOML="$REPO_ROOT/Cargo.toml"
README_MD="$REPO_ROOT/README.md"

# Get current version from Cargo.toml
get_version() {
    grep '^version = ' "$CARGO_TOML" | sed 's/version = "\(.*\)"/\1/'
}

# Set version in Cargo.toml
set_version() {
    local new_version="$1"
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/^version = .*/version = \"$new_version\"/" "$CARGO_TOML"
    else
        sed -i "s/^version = .*/version = \"$new_version\"/" "$CARGO_TOML"
    fi
    echo -e "${GREEN}✓${NC} Version updated to $new_version"
}

# Update version in README.md
update_readme_version() {
    local version="$1"
    if [[ "$OSTYPE" == "darwin"* ]]; then
        sed -i '' "s/^Current version: .*/Current version: ${version}/" "$README_MD"
    else
        sed -i "s/^Current version: .*/Current version: ${version}/" "$README_MD"
    fi
    echo -e "${GREEN}✓${NC} README version check complete"
}

# Print usage information
show_help() {
    cat << 'EOF'
Usage: scripts/manage.sh <command> [options]

Commands:
  clean              Remove build artifacts and caches
  build              Build in debug mode
  release            Build optimized release binary
  run                Run the debug binary (pass args: -- <args>)
  run-release        Run the release binary (builds it first if needed)
  test               Run tests
  lint               Run clippy linter
  fmt                Format code with rustfmt
  check              Quick syntax check (equivalent to cargo check)
  doc                Generate and open documentation
  
  version            Show current version
  bump               Bump patch version (0.1.0 -> 0.1.1)
  minor              Bump minor version (0.1.0 -> 0.2.0)
  major              Bump major version (0.1.0 -> 1.0.0)
  
  help               Show this message

Examples:
  ./scripts/manage.sh build
  ./scripts/manage.sh release
  ./scripts/manage.sh bump
  ./scripts/manage.sh run -- --help
  ./scripts/manage.sh minor

EOF
}

# Clean build artifacts
cmd_clean() {
    echo -e "${BLUE}→${NC} Cleaning build artifacts..."
    cd "$REPO_ROOT"
    cargo clean
    echo -e "${GREEN}✓${NC} Clean complete"
}

# Build debug binary
cmd_build() {
    echo -e "${BLUE}→${NC} Building debug binary..."
    cd "$REPO_ROOT"
    cargo build
    echo -e "${GREEN}✓${NC} Build complete"
}

# Build release binary
cmd_release() {
    echo -e "${BLUE}→${NC} Building release binary..."
    cd "$REPO_ROOT"
    cargo build --release --locked
    echo -e "${GREEN}✓${NC} Release build complete"
    echo -e "Binary location: ${BLUE}target/release/rate_mirrors${NC}"
}

# Run the binary
cmd_run() {
    echo -e "${BLUE}→${NC} Running binary..."
    cd "$REPO_ROOT"
    cargo run -- "$@"
}

# Run the release binary
cmd_run_release() {
    echo -e "${BLUE}→${NC} Running release binary..."
    cd "$REPO_ROOT"
    cargo run --release --locked -- "$@"
}

# Run tests
cmd_test() {
    echo -e "${BLUE}→${NC} Running tests..."
    cd "$REPO_ROOT"
    cargo test
    echo -e "${GREEN}✓${NC} Tests complete"
}

# Run clippy linter
cmd_lint() {
    echo -e "${BLUE}→${NC} Running clippy..."
    cd "$REPO_ROOT"
    cargo clippy --all-targets --all-features -- -D warnings
    echo -e "${GREEN}✓${NC} Linting complete"
}

# Format code
cmd_fmt() {
    echo -e "${BLUE}→${NC} Formatting code..."
    cd "$REPO_ROOT"
    cargo fmt --all
    echo -e "${GREEN}✓${NC} Formatting complete"
}

# Quick syntax check
cmd_check() {
    echo -e "${BLUE}→${NC} Checking syntax..."
    cd "$REPO_ROOT"
    cargo check
    echo -e "${GREEN}✓${NC} Check complete"
}

# Generate documentation
cmd_doc() {
    echo -e "${BLUE}→${NC} Generating documentation..."
    cd "$REPO_ROOT"
    cargo doc --no-deps --open
    echo -e "${GREEN}✓${NC} Documentation generated and opened"
}

# Show current version
cmd_version() {
    local version=$(get_version)
    echo "Current version: ${BLUE}$version${NC}"
}

# Bump patch version
cmd_bump() {
    local version=$(get_version)
    local new_version=$(echo "$version" | awk -F. '{print $1"."$2"."($3+1)}')
    set_version "$new_version"
    update_readme_version "$new_version"
    echo -e "${YELLOW}Bumped:${NC} $version → $new_version"
}

# Bump minor version
cmd_minor() {
    local version=$(get_version)
    local new_version=$(echo "$version" | awk -F. '{print $1".($2+1)".0}')
    set_version "$new_version"
    update_readme_version "$new_version"
    echo -e "${YELLOW}Bumped:${NC} $version → $new_version"
}

# Bump major version
cmd_major() {
    local version=$(get_version)
    local new_version=$(echo "$version" | awk -F. '{print ($1+1)".0.0"}')
    set_version "$new_version"
    update_readme_version "$new_version"
    echo -e "${YELLOW}Bumped:${NC} $version → $new_version"
}

# Main command handler
main() {
    if [[ $# -eq 0 ]]; then
        show_help
        exit 0
    fi

    local cmd="$1"
    shift

    case "$cmd" in
        clean)      cmd_clean ;;
        build)      cmd_build ;;
        release)    cmd_release ;;
        run)        cmd_run "$@" ;;
        run-release) cmd_run_release "$@" ;;
        test)       cmd_test ;;
        lint)       cmd_lint ;;
        fmt)        cmd_fmt ;;
        check)      cmd_check ;;
        doc)        cmd_doc ;;
        version)    cmd_version ;;
        bump)       cmd_bump ;;
        minor)      cmd_minor ;;
        major)      cmd_major ;;
        help|-h|--help) show_help ;;
        *)
            echo -e "${RED}✗${NC} Unknown command: $cmd"
            show_help
            exit 1
            ;;
    esac
}

main "$@"

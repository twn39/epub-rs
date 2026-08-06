#!/usr/bin/env bash
set -euo pipefail

# Helper script for running code coverage on epub-rs
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

cd "$PROJECT_DIR"

if ! command -v cargo-llvm-cov &> /dev/null; then
    echo "cargo-llvm-cov is not installed. Installing..."
    cargo install cargo-llvm-cov --locked
fi

echo "==> Running code coverage for epub-rs..."
cargo llvm-cov --all-features --workspace "$@"

echo ""
echo "==> Generating HTML coverage report (coverage/html/index.html)..."
cargo llvm-cov --all-features --workspace --html --output-dir coverage/html

echo ""
echo "==> Generating LCOV report (lcov.info)..."
cargo llvm-cov --all-features --workspace --lcov --output-path lcov.info

echo ""
echo "✅ Code coverage completed successfully!"
echo "   - HTML report: file://${PROJECT_DIR}/coverage/html/index.html"
echo "   - LCOV file:   file://${PROJECT_DIR}/lcov.info"

#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-3.0-or-later
# Copyright (C) 2024-2026 NitroSense Contributors
# scripts/coverage.sh — generate Rust line/branch coverage for the workspace.
#
# Usage:
#   scripts/coverage.sh           # HTML report at target/llvm-cov/html/
#   scripts/coverage.sh --open    # open the HTML report after generation
#   scripts/coverage.sh --lcov    # also emit lcov.info for CI ingestion
#   scripts/coverage.sh --summary # just print the line-coverage summary
#
# Hardware-gated tests are excluded by default because they require root and
# real Acer Nitro hardware. Add `--features hardware-tests` to opt in.

set -euo pipefail

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
    cat >&2 <<'EOF'
cargo-llvm-cov is not installed. Install it with:

    cargo install cargo-llvm-cov

Or use a package manager (Arch: pacman -S cargo-llvm-cov; nix has it too).
EOF
    exit 127
fi

OPEN=0
LCOV=0
SUMMARY_ONLY=0
EXTRA_ARGS=()

while [[ $# -gt 0 ]]; do
    case "$1" in
        --open)
            OPEN=1
            shift
            ;;
        --lcov)
            LCOV=1
            shift
            ;;
        --summary)
            SUMMARY_ONLY=1
            shift
            ;;
        *)
            EXTRA_ARGS+=("$1")
            shift
            ;;
    esac
done

ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT_DIR"

if [[ "$SUMMARY_ONLY" -eq 1 ]]; then
    cargo llvm-cov --workspace --summary-only "${EXTRA_ARGS[@]}"
    exit 0
fi

cargo llvm-cov clean --workspace
cargo llvm-cov --workspace --html "${EXTRA_ARGS[@]}"

if [[ "$LCOV" -eq 1 ]]; then
    cargo llvm-cov report --lcov --output-path target/llvm-cov/lcov.info "${EXTRA_ARGS[@]}"
    echo "lcov report at target/llvm-cov/lcov.info"
fi

echo "HTML coverage report at target/llvm-cov/html/index.html"

if [[ "$OPEN" -eq 1 ]]; then
    if command -v xdg-open >/dev/null 2>&1; then
        xdg-open target/llvm-cov/html/index.html >/dev/null 2>&1 &
    elif command -v open >/dev/null 2>&1; then
        open target/llvm-cov/html/index.html
    fi
fi

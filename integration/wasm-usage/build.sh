#!/usr/bin/env bash
# Build the yubiwallet-wasm npm package with wasm-pack.
#
#   ./build.sh [web|bundler|nodejs]   (default: web)
#
# Output goes to yubiwallet-wasm/pkg/ (npm-publishable). Requires wasm-pack:
#   cargo install wasm-pack   # or: https://rustwasm.github.io/wasm-pack/installer/
set -euo pipefail
TARGET="${1:-web}"
REPO_ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
cd "$REPO_ROOT"
wasm-pack build yubiwallet-wasm --release --target "$TARGET" --out-dir pkg
echo "Built yubiwallet-wasm/pkg (target=$TARGET). Publish with: (cd yubiwallet-wasm/pkg && npm publish)"

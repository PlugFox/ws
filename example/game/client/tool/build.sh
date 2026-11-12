#!/usr/bin/env bash
# Builds the web client into build/web (served by the game server at `/`).
#   tool/build.sh [--wasm]
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/web
cp web/index.html build/web/index.html
if [[ "${1:-}" == "--wasm" ]]; then
  dart compile wasm web/main.dart -o build/web/main.wasm
else
  dart compile js web/main.dart -O2 -o build/web/main.dart.js
fi

#!/usr/bin/env bash
# Builds the web client into build/web (served by the game server at `/`).
#   tool/build.sh [--wasm]
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p build/web
# Static files: the page, PWA manifest and service worker, brand images
# (baked by assets/pipeline/brand_baker.py). Not the Dart entry point.
find web -maxdepth 1 -type f ! -name '*.dart' -exec cp {} build/web/ \;
# Where the client finds the baked pack (`ASSETS_BASE`, default /assets;
# the Docker image sets /assets/v/<build> for long caching).
defines=(-DASSETS_BASE="${ASSETS_BASE:-/assets}")
if [[ "${1:-}" == "--wasm" ]]; then
  dart compile wasm "${defines[@]}" web/main.dart -o build/web/main.wasm
else
  dart compile js "${defines[@]}" web/main.dart -O2 -o build/web/main.dart.js
fi

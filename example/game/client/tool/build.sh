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
# The build the server checks on connect (`?build=` on /api/ws,
# server/src/version.rs): the commit's short hash, as the server's build.rs
# stamps it; the image passes it as GAME_BUILD (no .git in its context).
build_id=${GAME_BUILD:-$(git rev-parse --short=8 HEAD 2>/dev/null || echo dev)}
defines=(-DASSETS_BASE="${ASSETS_BASE:-/assets}" -DGAME_BUILD="$build_id")
if [[ "${1:-}" == "--wasm" ]]; then
  dart compile wasm "${defines[@]}" web/main.dart -o build/web/main.wasm
else
  # --no-deprecated-js-interop (Dart 3.14): fail on dart:html, dart:js_util
  # or package:js, the client uses dart:js_interop only.
  dart compile js "${defines[@]}" --no-deprecated-js-interop web/main.dart -O2 -o build/web/main.dart.js
fi

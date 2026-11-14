#!/usr/bin/env bash
# Writes <file>.br (brotli 11) and <file>.gz (gzip 9) beside every
# compressible static file under the given directories; the game server
# sends them to clients that accept them (server/src/api/static_files.rs).
# A variant that saves less than 10% is dropped. Variants keep the file's
# modification time, so Last-Modified is the same in every encoding.
#   deploy/precompress.sh DIR...
set -euo pipefail

compress() {
  local f size
  for f in "$@"; do
    size=$(wc -c <"$f")
    brotli -q 11 -f -o "$f.br" "$f"
    gzip -9 -n -c "$f" >"$f.gz"
    for v in "$f.br" "$f.gz"; do
      if (($(wc -c <"$v") * 10 > size * 9)); then rm -f "$v"; else touch -r "$f" "$v"; fi
    done
  done
}
export -f compress

# Text and geometry; images, audio and fonts are compressed formats already.
find "$@" -type f -size +1k \( -name '*.js' -o -name '*.mjs' -o -name '*.json' -o -name '*.css' \
  -o -name '*.html' -o -name '*.svg' -o -name '*.webmanifest' -o -name '*.wasm' -o -name '*.txt' \
  -o -name '*.xml' -o -name '*.glb' -o -name '*.bin' -o -name '*.ico' \) -print0 |
  xargs -0 -n 16 -P "$(getconf _NPROCESSORS_ONLN)" bash -c 'compress "$@"' _

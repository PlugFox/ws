#!/usr/bin/env bash
# Copies the ws_native package to a directory outside the repository so it
# can be published: the repository's root .pubignore hides native/ (it must
# not be part of the ws package), and pub applies parent ignore files.
#
# Usage: tool/stage_native.sh <out-dir> [ws-version]
#
# ws_native is released together with ws and pins it exactly: the FFI glue
# and the tape decoder live in ws, the binary here (distribution.md F10).
set -euo pipefail
out="${1:?usage: tool/stage_native.sh <out-dir> [ws-version]}"
ws_version="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
rm -rf "$out"
mkdir -p "$out"
rsync -a --exclude '.dart_tool' --exclude 'rust/target' --exclude 'pubspec.lock' \
  "$root/native/" "$out/"
cp "$root/LICENSE" "$out/LICENSE"
if [[ -n "$ws_version" ]]; then
  # Replace the path dependency on ws with the released version.
  python3 - "$out/pubspec.yaml" "$ws_version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
s = open(p).read()
s = re.sub(r"  ws:\n    path: \.\.\n", f"  ws: {v}\n", s)
s = s.replace("publish_to: none # until the first release\n", "")
open(p, "w").write(s)
PY
fi
echo "Staged ws_native in $out"

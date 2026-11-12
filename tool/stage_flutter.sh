#!/usr/bin/env bash
# Copies the ws_flutter package to a directory outside the repository so it
# can be published (the root .pubignore hides flutter/, and pub applies
# parent ignore files), as tool/stage_native.sh does for ws_native.
#
# Usage: tool/stage_flutter.sh <out-dir> [ws-version]
#
# Only files tracked by git are staged. With [ws-version] the path
# dependency on ws becomes `^<ws-version>`: ws_flutter uses only the public
# API, so any later 2.x works (unlike ws_native, D43).
set -euo pipefail
out="${1:?usage: tool/stage_flutter.sh <out-dir> [ws-version]}"
ws_version="${2:-}"
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
# Checked before anything is deleted.
case "$out/" in
  "$root"/*) echo "stage outside the repository, not in $out" >&2; exit 2 ;;
esac
rm -rf "$out"
mkdir -p "$out"

(
  cd "$root/flutter"
  git ls-files -z -- . | while IFS= read -r -d '' file; do
    mkdir -p "$out/$(dirname "$file")"
    cp "$file" "$out/$file"
  done
)
cp "$root/LICENSE" "$out/LICENSE"

if [[ -n "$ws_version" ]]; then
  python3 - "$out/pubspec.yaml" "$ws_version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
if not re.fullmatch(r"\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?", v):
    sys.exit(f"not a version: {v}")
s = open(p).read()
s, n = re.subn(r"  ws:\n    path: \.\.\n", f"  ws: ^{v}\n", s)
if n != 1:
    sys.exit("pubspec.yaml: the ws path dependency was not found")
s = s.replace("publish_to: none # until the first release\n", "")
open(p, "w").write(s)
PY
fi
echo "Staged ws_flutter in $out"

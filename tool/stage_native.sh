#!/usr/bin/env bash
# Copies the ws_native package to a directory outside the repository so it
# can be published: the repository's root .pubignore hides native/ (it must
# not be part of the ws package), and pub applies parent ignore files.
#
# Usage: tool/stage_native.sh <out-dir> [ws-version] [--release <notices>]
#
# Only files tracked by git are staged, plus native/prebuilt/ (built by CI),
# so untracked files in the working tree never reach pub.dev
# (distribution.md F19). Tests and Rust tooling configuration stay behind.
#
# ws_native is released together with ws and allows only that version: the FFI glue
# and the tape decoder live in ws, the binary here (distribution.md F10).
#
# --release <notices>: the prebuilt libraries must be exactly the 12
# targets and match checksums.json, and the third-party notices
# (tool/third_party_licenses.sh) are appended to LICENSE, where Flutter's
# LicenseRegistry shows them (distribution.md F8).
set -euo pipefail
out="${1:?usage: tool/stage_native.sh <out-dir> [ws-version] [--release <notices>]}"
shift
ws_version=""
notices=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --release) notices="${2:?--release needs the notices file}"; shift 2 ;;
    *) ws_version="$1"; shift ;;
  esac
done
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$out"
out="$(cd "$out" && pwd)"
# Checked before anything is deleted: `rm -rf native` must never happen.
case "$out/" in
  "$root"/*) echo "stage outside the repository, not in $out" >&2; exit 2 ;;
esac
rm -rf "$out"
mkdir -p "$out"

(
  cd "$root/native"
  git ls-files -z -- . \
    ':!:test/**' ':!:rust/rust-toolchain.toml' ':!:rust/deny.toml' \
    ':!:rust/about.toml' ':!:rust/about.hbs' |
    while IFS= read -r -d '' file; do
      mkdir -p "$out/$(dirname "$file")"
      cp "$file" "$out/$file"
    done
)
if [[ -d "$root/native/prebuilt" ]]; then
  cp -R "$root/native/prebuilt" "$out/prebuilt"
fi
cp "$root/LICENSE" "$out/LICENSE"

if [[ -n "$notices" ]]; then
  [[ -s "$notices" ]] || { echo "missing notices: $notices" >&2; exit 1; }
  (cd "$root" && dart run tool/native_checksums.dart --check "$out/prebuilt")
  # Flutter's LicenseRegistry splits LICENSE files on a line of 80 hyphens.
  {
    echo
    printf '%.0s-' {1..80}
    echo
    cat "$notices"
  } >>"$out/LICENSE"
fi

if [[ -n "$ws_version" ]]; then
  # Replace the path dependency on ws with the released version.
  python3 - "$out/pubspec.yaml" "$ws_version" <<'PY'
import re, sys
p, v = sys.argv[1], sys.argv[2]
# Exactly this version, written as a range: pub warns about a single-version
# constraint. Nothing exists between v and the bound: for 2.1.0 the bound is
# 2.1.1 (pub excludes 2.1.1-dev.x), for 2.1.0-dev.3 it is 2.1.0-dev.3.0.
m = re.fullmatch(r"(\d+)\.(\d+)\.(\d+)(-[0-9A-Za-z.]+)?", v)
if not m:
    sys.exit(f"not a version: {v}")
major, minor, patch, pre = m.groups()
bound = f"{v}.0" if pre else f"{major}.{minor}.{int(patch) + 1}"
s = open(p).read()
s, n = re.subn(r"  ws:\n    path: \.\.\n", f"  ws: '>={v} <{bound}'\n", s)
if n != 1:
    sys.exit("pubspec.yaml: the ws path dependency was not found")
s = s.replace("publish_to: none # until the first release\n", "")
open(p, "w").write(s)
PY
fi
echo "Staged ws_native in $out"

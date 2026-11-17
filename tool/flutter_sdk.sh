#!/usr/bin/env bash
# Installs a stable Flutter SDK for Linux x64 (CI) into <dir>/flutter and
# adds it to GITHUB_PATH when set. The archive's SHA-256 is checked against
# Flutter's release manifest; no third-party action is involved.
#
# Usage: tool/flutter_sdk.sh <version> <dir>
set -euo pipefail
version="${1:?usage: tool/flutter_sdk.sh <version> <dir>}"
dir="${2:?usage: tool/flutter_sdk.sh <version> <dir>}"
base="https://storage.googleapis.com/flutter_infra_release/releases"
archive="stable/linux/flutter_linux_${version}-stable.tar.xz"
sha="$(curl -fsSL --retry 3 "$base/releases_linux.json" |
  jq -r --arg a "$archive" '.releases[] | select(.archive == $a) | .sha256')"
if [[ ! "$sha" =~ ^[0-9a-f]{64}$ ]]; then
  echo "Flutter $version is not in the release manifest" >&2
  exit 1
fi
mkdir -p "$dir"
tmp="$(mktemp)"
trap 'rm -f "$tmp"' EXIT
curl -fsSL --retry 3 -o "$tmp" "$base/$archive"
echo "$sha  $tmp" | sha256sum -c --quiet
tar -xJf "$tmp" -C "$dir"
"$dir/flutter/bin/flutter" --disable-analytics >/dev/null
"$dir/flutter/bin/flutter" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then
  echo "$dir/flutter/bin" >>"$GITHUB_PATH"
fi

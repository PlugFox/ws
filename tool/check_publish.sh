#!/usr/bin/env bash
# pub publish dry runs: ws from the repository, ws_native from a staged copy
# outside it (DECISIONS.md D25), resolved against this checkout of ws.
# A dirty git tree makes the ws dry run report a warning and fail.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
version="$(awk '/^version:/ {print $2}' pubspec.yaml)"
tool/stage_native.sh "$stage/ws_native" "$version"
printf 'dependency_overrides:\n  ws:\n    path: %s\n' "$root" \
  >"$stage/ws_native/pubspec_overrides.yaml"
(cd "$stage/ws_native" && dart pub get >/dev/null && dart pub publish --dry-run)
dart pub publish --dry-run

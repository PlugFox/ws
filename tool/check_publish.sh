#!/usr/bin/env bash
# pub publish dry runs: ws from the repository, ws_native from a staged copy
# outside it (DECISIONS.md D25), resolved against this checkout of ws.
#
# Warnings (a dirty tree during development, the example's asset
# submodule) are printed but tolerated locally; on CI (CI=true) they fail,
# as do errors anywhere (distribution.md F27).
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
version="$(awk '/^version:/ {print $2}' pubspec.yaml)"
tool/stage_native.sh "$stage/ws_native" "$version"
if [[ ! -d native/prebuilt ]]; then
  echo "note: no native/prebuilt: the ws_native archive has no libraries," \
    "so its size and contents are not the release's" >&2
fi
printf 'dependency_overrides:\n  ws:\n    path: %s\n' "$root" \
  >"$stage/ws_native/pubspec_overrides.yaml"

# Runs a dry run in $1; fails on errors, and on warnings on CI.
dry_run() {
  local out status=0
  out="$(cd "$1" && dart pub publish --dry-run 2>&1)" || status=$?
  printf '%s\n' "$out"
  [[ $status == 0 ]] && return 0
  if grep -qE '^Package has [0-9]+ warnings?\.$' <<<"$out" &&
    ! grep -qE 'Package has [0-9]+ errors?' <<<"$out" &&
    [[ "${CI:-}" != true ]]; then
    echo "dry run in $1: warnings only, tolerated outside CI" >&2
    return 0
  fi
  return "$status"
}
(cd "$stage/ws_native" && dart pub get >/dev/null)
dry_run "$stage/ws_native"
dry_run "$root"

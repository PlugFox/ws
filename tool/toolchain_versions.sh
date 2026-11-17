#!/usr/bin/env bash
# Fails when the pinned toolchain versions drift apart: the Dart SDK of
# mise.toml, the format/analyze job (checkout.yml), the release workflow and
# the game image; the lowest Flutter of flutter/pubspec.yaml and the CI
# matrix (the release builds no Flutter package, D58); the Rust of mise.toml
# and the game image (tool/rust_toolchain.sh checks the
# rust-toolchain.toml files); the Node, Python and uv of mise.toml and the
# image.
#
# Usage: tool/toolchain_versions.sh
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
mise="$root/mise.toml"
checkout="$root/.github/workflows/checkout.yml"
release="$root/.github/workflows/release.yml"
docker="$root/example/game/deploy/Dockerfile"

first() { sed -n "$1" "$2" | head -n 1; }
status=0
same() { # name, then "where=value" pairs; every value must be equal and non-empty
  local name="$1" want="" pair
  shift
  for pair in "$@"; do
    local value="${pair#*=}"
    if [[ -z "$value" ]]; then
      echo "$name: no version found in ${pair%%=*}" >&2
      status=1
    elif [[ -z "$want" ]]; then
      want="$value"
    elif [[ "$value" != "$want" ]]; then
      echo "$name versions differ: $*" >&2
      status=1
      return
    fi
  done
  echo "$name $want"
}

same Dart \
  "mise.toml=$(first 's/^dart = { version = "\([^"]*\)".*/\1/p' "$mise")" \
  "checkout.yml=$(first 's/^ *sdk: \([0-9][0-9.]*\)$/\1/p' "$checkout")" \
  "release.yml=$(first 's/^ *DART_VERSION: \([0-9.]*\).*/\1/p' "$release")" \
  "Dockerfile=$(first 's/^ARG DART_VERSION=\(.*\)$/\1/p' "$docker")"

# The lowest Flutter the recipe package declares must be tested in CI.
flutter="$(first 's/^ *flutter: ">=\([0-9.]*\)"$/\1/p' "$root/flutter/pubspec.yaml")"
matrix="$(first 's/^ *flutter: \[\(.*\)\]$/\1/p' "$checkout")"
if [[ -z "$flutter" || ", $matrix, " != *", $flutter, "* ]]; then
  echo "Flutter $flutter (flutter/pubspec.yaml) is not in the checkout.yml matrix [$matrix]" >&2
  status=1
else
  echo "Flutter $flutter (lowest) in [$matrix]"
fi

same Rust \
  "mise.toml=$(first 's/^rust = { version = "\([^"]*\)".*/\1/p' "$mise")" \
  "Dockerfile=$(first 's/^ARG RUST_VERSION=\(.*\)$/\1/p' "$docker")"

same Node \
  "mise.toml=$(first 's/^node = "\([^"]*\)"$/\1/p' "$mise")" \
  "Dockerfile=$(first 's/^ARG NODE_VERSION=\(.*\)$/\1/p' "$docker")"

same Python \
  "mise.toml=$(first 's/^python = "\([^"]*\)"$/\1/p' "$mise")" \
  "Dockerfile=$(first 's/^ARG PYTHON_VERSION=\(.*\)$/\1/p' "$docker")"

# uv: mise pins a minor ("0.13"), the image an exact release of it.
uv_mise="$(first 's/^uv = "\([^"]*\)"$/\1/p' "$mise")"
uv_image="$(first 's/^ARG UV_VERSION=\(.*\)$/\1/p' "$docker")"
if [[ -z "$uv_mise" || "$uv_image" != "$uv_mise" && "$uv_image" != "$uv_mise".* ]]; then
  echo "uv versions differ: mise.toml $uv_mise, Dockerfile $uv_image" >&2
  status=1
else
  echo "uv $uv_image"
fi

exit "$status"

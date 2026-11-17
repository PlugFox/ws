#!/usr/bin/env bash
# Installs the Rust toolchain pinned in rust-toolchain.toml (CI), after
# checking that native/rust, server and mise.toml pin the same version
# (distribution.md F15). Extra arguments are targets to add.
#
# Usage: tool/rust_toolchain.sh [target-triple...]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
channel() { sed -n 's/^channel = "\(.*\)"$/\1/p' "$1"; }
native="$(channel "$root/native/rust/rust-toolchain.toml")"
server="$(channel "$root/server/rust-toolchain.toml")"
mise="$(sed -n 's/^rust = { version = "\([^"]*\)".*/\1/p' "$root/mise.toml")"
if [[ -z "$native" || "$native" != "$server" || "$native" != "$mise" ]]; then
  echo "Rust versions differ: native/rust $native, server $server, mise.toml $mise" >&2
  exit 1
fi
cd "$root/native/rust"
rustup toolchain install "$native" --profile minimal --component rustfmt,clippy \
  ${1:+--target "$(IFS=,; echo "$*")"}
rustc --version

#!/usr/bin/env bash
# Writes the licence notices of every Rust crate in libws_native
# (native/rust/about.toml, about.hbs). The release ships them as a GitHub
# release asset and appended to ws_native's LICENSE (distribution.md F8).
# Needs cargo-about: `cargo install cargo-about --locked --features cli`.
#
# Usage: tool/third_party_licenses.sh <out-file>
set -euo pipefail
out="${1:?usage: tool/third_party_licenses.sh <out-file>}"
root="$(cd "$(dirname "$0")/.." && pwd)"
case "$out" in /*) ;; *) out="$PWD/$out" ;; esac
mkdir -p "$(dirname "$out")"
cd "$root/native/rust"
cargo about generate --locked --fail about.hbs -o "$out"
echo "$(grep -c '^- ' "$out") crate licences in $out"

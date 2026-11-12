#!/usr/bin/env bash
# Builds libws_native for several targets into native/prebuilt/ and writes
# checksums.json. Without arguments: every target this machine can build
# (Apple targets on macOS, Android when an NDK is found, the host on
# Linux and Windows). Linux and Windows libraries for release come from CI:
# `mise run ci:prebuilt`.
#
# Usage: tool/prebuilt.sh [target...]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

if [[ -z "${ANDROID_NDK_HOME:-}" && -d "$HOME/Library/Android/sdk/ndk" ]]; then
  # Newest NDK of a default Android Studio install.
  ANDROID_NDK_HOME="$(find "$HOME/Library/Android/sdk/ndk" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)"
  export ANDROID_NDK_HOME
fi

targets=("$@")
if [[ ${#targets[@]} -eq 0 ]]; then
  case "$(uname -s)" in
    Darwin) targets=(macos-arm64 macos-x64 ios-arm64 ios-arm64-sim ios-x64-sim) ;;
    Linux) targets=("linux-$([[ "$(uname -m)" == aarch64 ]] && echo arm64 || echo x64)") ;;
    *) targets=(windows-x64) ;;
  esac
  if [[ -n "${ANDROID_NDK_HOME:-}" ]]; then
    targets+=(android-arm64 android-arm android-x64)
  fi
fi

for target in "${targets[@]}"; do
  echo "== $target"
  tool/build_native.sh "$target"
done
dart run tool/native_checksums.dart native/prebuilt

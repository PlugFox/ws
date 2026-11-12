#!/usr/bin/env bash
# Builds libws_native for one target into native/prebuilt/<target>/, the
# layout the build hook and `dart run ws:install` expect (DECISIONS.md D9,
# D32). Used locally (`mise run native:prebuilt`) and by
# .github/workflows/prebuilt.yml.
#
# Usage: tool/build_native.sh <target> [out-dir]
#   target: macos-arm64 macos-x64 ios-arm64 ios-arm64-sim ios-x64-sim
#           android-arm64 android-arm android-x64 linux-x64 linux-arm64
#           windows-x64 windows-arm64
#   out-dir: default native/prebuilt
#
# Minimum OS versions stay at or below Flutter's (iOS 13.0, macOS 10.15,
# Android API 21): the binary's minimum must not exceed the app's.
# Android needs ANDROID_NDK_HOME (or ANDROID_NDK_LATEST_HOME on CI runners).
# Linux targets link against glibc 2.17 through cargo-zigbuild when it is
# installed (CI does), so the library loads on old distributions
# (distribution.md F3); libgcc is linked statically (native/rust/build.rs).
set -euo pipefail
target="${1:?usage: tool/build_native.sh <target> [out-dir]}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="${2:-$root/native/prebuilt}"
android_api=21

case "$target" in
  macos-arm64)   triple=aarch64-apple-darwin ;;
  macos-x64)     triple=x86_64-apple-darwin ;;
  ios-arm64)     triple=aarch64-apple-ios ;;
  ios-arm64-sim) triple=aarch64-apple-ios-sim ;;
  ios-x64-sim)   triple=x86_64-apple-ios ;;
  android-arm64) triple=aarch64-linux-android ;;
  android-arm)   triple=armv7-linux-androideabi ;;
  android-x64)   triple=x86_64-linux-android ;;
  linux-x64)     triple=x86_64-unknown-linux-gnu ;;
  linux-arm64)   triple=aarch64-unknown-linux-gnu ;;
  windows-x64)   triple=x86_64-pc-windows-msvc ;;
  windows-arm64) triple=aarch64-pc-windows-msvc ;;
  *) echo "unknown target: $target" >&2; exit 2 ;;
esac

case "$target" in
  windows-*) file=ws_native.dll ;;
  macos-* | ios-*) file=libws_native.dylib ;;
  *) file=libws_native.so ;;
esac

case "$target" in
  macos-*) export MACOSX_DEPLOYMENT_TARGET=10.15 ;;
  ios-*) export IPHONEOS_DEPLOYMENT_TARGET=13.0 ;;
  android-*)
    ndk="${ANDROID_NDK_HOME:-${ANDROID_NDK_LATEST_HOME:-}}"
    [[ -n "$ndk" ]] || { echo "set ANDROID_NDK_HOME" >&2; exit 2; }
    bin="$(find "$ndk/toolchains/llvm/prebuilt" -mindepth 2 -maxdepth 2 -name bin | head -1)"
    prefix="$triple"
    [[ "$triple" == armv7-linux-androideabi ]] && prefix=armv7a-linux-androideabi
    key="$(echo "$triple" | tr 'a-z-' 'A-Z_')"
    cc_key="${triple//-/_}"
    export "CARGO_TARGET_${key}_LINKER=$bin/$prefix$android_api-clang"
    export "CC_${cc_key}=$bin/$prefix$android_api-clang"
    export "AR_${cc_key}=$bin/llvm-ar"
    ;;
esac

# In native/rust, so the target goes to the toolchain rust-toolchain.toml pins.
(cd "$root/native/rust" && rustup target add "$triple" >/dev/null 2>&1) || true
build=(cargo build --release --locked --target "$triple")
case "$target" in
  linux-*)
    if cargo zigbuild --help >/dev/null 2>&1; then
      build=(cargo zigbuild --release --locked --target "$triple.2.17")
    else
      echo "warning: cargo-zigbuild not found; glibc floor = this machine" >&2
    fi
    ;;
esac
(cd "$root/native/rust" && "${build[@]}")
mkdir -p "$out/$target"
cp "$root/native/rust/target/$triple/release/$file" "$out/$target/$file"
echo "$out/$target/$file"

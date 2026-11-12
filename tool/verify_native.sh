#!/usr/bin/env bash
# Checks a built libws_native before it is shipped (distribution.md F9):
#   - exactly the 13 exported ws_native_* functions;
#   - Android: every LOAD segment aligned to 16 KB (Google Play);
#   - Linux: glibc symbols <= 2.17 and only libc/libm/libdl/libpthread/ld
#     needed (no libgcc_s, F2/F3);
#   - Apple: install name @rpath/libws_native.dylib;
#   - Windows: no VC++ runtime import (static CRT, F17), when objdump exists.
#
# Usage: tool/verify_native.sh <target> [dir]   (dir: default native/prebuilt)
set -euo pipefail
target="${1:?usage: tool/verify_native.sh <target> [dir]}"
root="$(cd "$(dirname "$0")/.." && pwd)"
dir="${2:-$root/native/prebuilt}"
case "$target" in
  windows-*) file="$dir/$target/ws_native.dll" ;;
  macos-* | ios-*) file="$dir/$target/libws_native.dylib" ;;
  *) file="$dir/$target/libws_native.so" ;;
esac
[[ -f "$file" ]] || { echo "missing $file" >&2; exit 1; }
fail() { echo "verify $target: $*" >&2; exit 1; }
expected=13

case "$target" in
  android-* | linux-*)
    exports=$(nm -D --defined-only "$file" | awk '$2 == "T" {print $3}' | grep -c '^ws_native_' || true)
    others=$(nm -D --defined-only "$file" | awk '$2 == "T" {print $3}' | grep -vc '^ws_native_' || true)
    [[ "$exports" == "$expected" ]] || fail "$exports ws_native_* exports, expected $expected"
    [[ "$others" == 0 ]] || fail "$others other exported functions"
    ;;
  macos-* | ios-*)
    exports=$(nm -gU "$file" | awk '$2 == "T" {print $3}' | grep -c '^_ws_native_' || true)
    [[ "$exports" == "$expected" ]] || fail "$exports ws_native_* exports, expected $expected"
    otool -D "$file" | grep -q '^@rpath/libws_native.dylib$' || fail "install name is not @rpath/libws_native.dylib"
    otool -l "$file" | grep -A4 -E 'LC_BUILD_VERSION|LC_VERSION_MIN' | grep -E 'minos|version' | head -2
    ;;
esac

case "$target" in
  android-*)
    bad=$(readelf -lW "$file" | awk '$1 == "LOAD" && $NF != "0x4000" {print $NF}')
    [[ -z "$bad" ]] || fail "LOAD segments not 16 KB aligned: $bad"
    ;;
  linux-*)
    max=$(objdump -T "$file" | grep -o 'GLIBC_[0-9.]*' | sort -uV | tail -1)
    echo "highest glibc symbol: $max"
    [[ "$(printf '%s\nGLIBC_2.17\n' "$max" | sort -V | tail -1)" == GLIBC_2.17 ]] \
      || fail "needs $max, floor is GLIBC_2.17"
    needed=$(readelf -dW "$file" | awk '/NEEDED/ {gsub(/[\[\]]/, "", $NF); print $NF}')
    for lib in $needed; do
      case "$lib" in
        libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | ld-linux*) ;;
        *) fail "unexpected NEEDED $lib" ;;
      esac
    done
    ;;
  windows-*)
    if command -v objdump >/dev/null 2>&1; then
      if objdump -p "$file" | grep -qi 'vcruntime'; then
        fail "imports the VC++ runtime (build with +crt-static)"
      fi
    else
      echo "objdump not found: imports not checked"
    fi
    ;;
esac
echo "verified $file"

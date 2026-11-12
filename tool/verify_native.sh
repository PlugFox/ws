#!/usr/bin/env bash
# Checks a built libws_native before it is shipped (distribution.md F9):
#   - exactly the 13 exported ws_native_* functions;
#   - Android: every LOAD segment aligned to 16 KB (Google Play);
#   - Linux: glibc symbols <= 2.17 and only libc/libm/libdl/libpthread/ld
#     needed (no libgcc_s, F2/F3);
#   - Apple: install name @rpath/libws_native.dylib, minimum OS at most
#     Flutter's, iOS required-reason APIs as declared in the README;
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
    # The binary's minimum OS must not exceed the app's, or App Store
    # validation rejects the bundle (Flutter's generated framework says
    # 13.0 or later). Rust's own floors: macOS arm64 11.0, iOS arm64
    # simulator 14.0 (distribution.md F28).
    case "$target" in
      macos-arm64) floor=11.0 ;;
      macos-x64) floor=10.15 ;;
      ios-arm64-sim) floor=14.0 ;;
      *) floor=13.0 ;;
    esac
    minos=$(otool -l "$file" | awk '/LC_BUILD_VERSION/ {b = 1} b && $1 == "minos" {print $2; exit}')
    echo "minos: $minos (at most $floor)"
    [[ "$(printf '%s\n%s\n' "$minos" "$floor" | sort -V | tail -1)" == "$floor" ]] \
      || fail "minos $minos is above $floor"
    if [[ "$target" == ios-* ]]; then
      # Apple's "required reason" APIs; the README's privacy manifest
      # declares the file-timestamp reason for fstat (F23). A new one
      # needs a README update first.
      reasons=$(nm -u "$file" | grep -oE '^_(stat|lstat|fstat|fstatat|getattrlist|getattrlistbulk|fgetattrlist|statfs|fstatfs|statvfs|fstatvfs|mach_absolute_time)$' | sort -u | tr '\n' ' ')
      [[ "$reasons" == "_fstat " ]] || fail "required-reason APIs changed: $reasons(native/README.md)"
    fi
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

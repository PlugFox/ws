# Shipping native code: how other Dart packages do it

Survey made in November 2026 (Dart 3.13.7, SDK `main`) to choose how
`ws_native` ships its Rust library. Sources were package archives unpacked
with `dart pub unpack`, the dart-lang/native, pub and pub-dev repositories,
and GitHub issues. Archive sizes are the `.tar.gz` sizes on pub.dev.
Conclusions are in DECISIONS.md D8, D9, D30 and D31.

## Four ways to ship native code

| Approach | Packages | Integrity | What users run into |
|---|---|---|---|
| Hook downloads a prebuilt library | `sqlite3` 3.x, `powersync`, `llamadart`, `cbl` 4, `flutter_soloud` | SHA-256 pinned in Dart code: `sqlite3`, `powersync`, `llamadart`. None: `cbl`, `flutter_soloud` | `sqlite3` issues: Windows `CERTIFICATE_VERIFY_FAILED` (#344, #354), corporate proxy (#335), no network in the Nix sandbox (#346, open), glibc 2.38 required (#405) |
| Hook compiles from source | `sodium`, `webcrypto` (17 MB, vendored BoringSSL), `dartcv4`, `objective_c`, `native_toolchain_rust` users | — | Every user needs a toolchain. `super_native_extensions` moved from cargokit (prebuilt) to `native_toolchain_rust` on 2026-08-07 (#595), and five days later had #598 "failed to invoke rustup" |
| Flutter plugin with a native build system | `sqlite3_flutter_libs` (now an empty EOL stub), `objectbox_flutter_libs`, `isar_flutter_libs` (16.6 MB, bundles every platform), `isar_plus_flutter_libs`, `rhttp` (cargokit) | Mostly none: CMake `FetchContent` without `URL_HASH`. cargokit signs prebuilts with ed25519 | Flutter only. Pure-Dart users need a separate path |
| Manual install step for pure Dart | `isar` 3 `initializeIsarCore(download: true)`, `objectbox` `install.sh`, `realm_dart install`, `cbl_dart` 3, `rive_native` `bin/setup.dart` | None | "Can't find `libisar.so` in Docker" (isar#1465). Infosec teams reject downloads during the build (realm-dart#1703, "Dealbreaker") |

Further findings:

- **No published package bundles prebuilt binaries in its archive through a
  hook.** The Dart team does show that pattern, though: the
  `prebuilt_assets_example` added to dart-lang/native in 2026 does what
  `ws_native` does. CI builds `prebuilt/`, `.gitignore` excludes it, the
  archive includes it, and the hook builds from source when the file is
  missing or `local_build: true` is set. It has no hash check of its own,
  because `pubspec.lock` already pins the archive hash.
- **The Dart team's position on binaries:**
  - 2024 (mosuem, pub#3693): pub.dev will not host binaries "anytime soon";
    use GitHub releases plus a hash check.
  - 2025-07 (dcharkes): packages should offer a user-define to choose
    between a local build and a prebuilt library.
  - 2025-11 (jonasfj): "increasingly convinced that storing binary artifacts
    on pub.dev is not a bad idea". Nothing is scheduled.
  - There are no per-platform archives. An `AssetDownloader` helper
    (native#996) and an offline/prefetch mode for hooks (native#1620) are
    open.
- **pub limits:** pub-dev's
  `upload_signer_service.dart:56` and `pub_package_reader.dart:87-94`
  enforce at most 100 MiB compressed, 256 MiB uncompressed and 65,536 files
  per archive. Neither pub nor pana penalises `.so`, `.dylib` or `.dll`
  files.
- **pub client caveats for binaries:**
  - The leak-detection validator reads every file as text, so random bytes
    can match a "secret". Excluding files with `false_secrets` in the
    pubspec fixes that.
  - The gitignore validator warns only about files that are gitignored
    *and* committed. `prebuilt/` is never committed.
- **Network inside hooks:** the runner passes `http(s)_proxy`, `no_proxy`,
  `CARGO_*`, `RUSTUP_*`, `NIX_*`, `PATH` and `HOME` into hooks
  (`hooks_runner/.../build_runner.dart:616-680`). `sqlite3` needed Dart 3.11
  before proxies worked.
- **`sqlite3` 3.x dropped `DynamicLibrary`.**
  - It removed `open.overrideFor`, so the library can now be chosen only
    through user-defines.
  - Its maintainer's answer for `dart compile exe` users is `dart build cli`
    (sqlite3.dart#342). It has no Docker guide.
  - `sqflite_common_ffi` 2.3.7 deleted the `sqlite3.dll` it used to bundle.
    It tells users who have trouble with hooks to pin `sqlite3: ^2.9.4`.
  - Its user-defines accept a per-OS map, for example
    `source: {default: sqlite3, linux: system}` (since 3.6.0 and 3.7.0).
    That is worth copying.

## How Flutter treats a code asset per target

Source: `flutter_tools/lib/src/isolated/native_assets/`.

- **Apple.**
  - Each dylib is wrapped in `<name>.framework`, its install name is
    rewritten with `install_name_tool`, and in release it is stripped and
    codesigned. Architectures are combined with `lipo`.
  - Files are grouped by asset id and the framework is named after the file,
    so a file name that differs between architectures makes Flutter warn and
    drop one of them.
  - `ws_native` uses `libws_native.dylib` on every Apple target and links
    with `-headerpad_max_install_names` (`rust/build.rs`).
  - Device arm64 and simulator arm64 cannot share one lipo file, and the
    hook keeps them separate (`-sim` targets).
- **Android.** Files are copied unchanged into `jniLibs/<abi>/`. Flutter
  does not enforce 16 KB pages, and `native_toolchain_c` adds the flag
  itself, so a Rust library has to pass it too. `ws_native` does, in
  `rust/build.rs`.
- **Linux and Windows.** Files land flat in the bundle under their own
  name, so the name has to be unique across packages. Linux has no symbol
  namespaces, so the library should export as little as possible. The
  macOS build of `libws_native` exports 11 symbols, all prefixed
  `ws_native_` (`nm -gU`). A Rust `cdylib` exports only `#[no_mangle]`
  items on Linux as well; that is rustc behaviour and has not been
  re-checked on a Linux build.

# Review: packaging and delivery of `ws` 2.0 and `ws_native`

Date: 2026-11-12. Branch `rewrite`, HEAD `50bec2a` plus the uncommitted
working tree. Tools: Dart 3.13.7, Flutter 3.47.9, Xcode SDK 26.5, Docker
27.4 (aarch64), actionlint 1.7.12, shellcheck 0.11.

Scope: `pubspec.yaml`, `.pubignore`, `native/` (package, hook, prebuilt
layout), `lib/native.dart`, `dart run ws:install`, the release generators,
`tool/*.sh`, `mise.toml`, `.github/workflows/*`, README and CHANGELOG.

## What was run

Everything marked **verified** below was observed in one of these runs.
**Suspected** means it follows from reading the code or from knowledge of
the tooling and was not reproduced.

| Check | Result |
|---|---|
| `actionlint`, `shellcheck tool/*.sh` | Both exit 0, no findings |
| `gh run list --branch rewrite` | `Prebuilt` green (12 targets), `Native` green, **`Checkout` red** (two jobs, see F4) |
| `dart pub publish --dry-run` (ws) | 392 KB archive, one warning (dirty tree only) |
| Staged `ws_native` with the 12 CI libraries, `dart pub publish --dry-run` | 13 MB archive, 0 warnings, no false "secret" hits on the binaries |
| CI artifact `prebuilt` of run 43237370258, inspected with `file`, `objdump -p/-T`, `otool`, `vtool`, `codesign`, `nm` | See F2, F3, F9, F17 and "Verified to be fine" |
| Scratch Dart app on staged `ws_native`: `dart run`, `dart build cli` (host, `--target-os linux --target-arch x64`, `linux arm`), `dart compile exe` | Hook picks the prebuilt, checksum verified, cross build bundles the Linux library, unsupported target gives no asset, `compile exe` refuses as documented |
| Tampered prebuilt (one byte appended) | Hook fails loudly with the checksum message |
| User-defines: `source: none`, `source: prebuilt`, `library:`, unknown value, map value | See F12 |
| Scratch app on `ws` only: `dart compile exe`, `dart compile js`, `dart compile wasm`, `dart run ws:install` variants | Compile targets fine; installer findings F1, F13 |
| Docker: README flow in `dart:stable`, then `FROM scratch` + `/runtime/`, then `debian:bullseye-slim` | Loads in the build image; fails in the other two (F2, F3) |
| Scratch Flutter app (`ws` + staged `ws_native`): `flutter build macos`, `apk`, `ios --no-codesign`, `web` | macOS, Android, iOS build and bundle the library; macOS app launched and printed `install=true`; **web fails** (F5) |
| `cargo metadata` licence listing, `cargo audit` on `native/rust/Cargo.lock` | 107 crates, no copyleft, no advisories; no notices shipped (F8) |
| `tool/native_release.dart` against the CI `checksums.json` | Output is `dart format` clean; missing input silently writes an empty map (F7) |

Not verified: a run on a physical Android or iOS device, an App Store or
Play upload, any Windows or Linux desktop run of the hook path, Dart 3.10
(the declared lower bound), `pana` scores.

## Findings

Severity: **High** = users following the documentation hit a failure, or a
release would ship something wrong. **Medium** = fails for a real class of
users or weakens the release chain. **Low** = polish and hygiene.

| # | Sev | Where | Failure scenario | Fix | Status |
|---|---|---|---|---|---|
| F1 | High | `bin/install.dart:36`; `README.md:111`; `docs/DECISIONS.md` D32 | The documented command `dart run ws:install --target linux-x64 --out /app/lib` exits 1 with `Unknown argument: linux-x64`. The parser accepts only `--key=value`. Every Dockerfile copied from the README fails at this step. | Accept both `--key value` and `--key=value` (or use `package:args` semantics by hand), and add a test that runs the exact README line. | Verified (ran it) |
| F2 | High | `tool/build_native.sh:61`; `.github/workflows/prebuilt.yml:41-42` | `linux-*/libws_native.so` has `NEEDED libgcc_s.so.1`. The canonical Dart server image (`FROM scratch` + `COPY --from=build /runtime/ /`) has no `libgcc_s`, so `WsNativeLibrary.open` throws `libgcc_s.so.1: cannot open shared object file`. This is exactly the Docker use case D32 was written for. NixOS has the same problem for the bundled library (silent fallback there). | Build Linux targets with `cargo-zigbuild` (also fixes F3), then assert in CI that `NEEDED` is a subset of `libc.so.6`, `libm.so.6`, `libdl.so.2`, `libpthread.so.0`, `ld-linux-*`. Until then, document `COPY --from=build /lib/*/libgcc_s.so.1`. | Verified (Docker run) |
| F3 | High | `.github/workflows/prebuilt.yml:39-42`; `docs/DEVELOPMENT.md:29-30` | Linux libraries require `GLIBC_2.34` (`objdump -T`). On Debian 11 the Dart executable runs but the library fails with ``version `GLIBC_2.33' not found``. Also excluded: Ubuntu 20.04, RHEL/Alma/Rocky 8, Amazon Linux 2. The floor is whatever the oldest GitHub runner has, so it jumps again when `ubuntu-22.04` is retired. D9 says "pin a glibc floor (cargo-zigbuild)"; that was not implemented. DEVELOPMENT.md says 2.35; the measured value is 2.34. | `cargo zigbuild --target x86_64-unknown-linux-gnu.2.17` (or `.2.28`) for both Linux targets; add a CI step that fails when the highest `GLIBC_x.y` symbol exceeds the floor. | Verified (Docker run, objdump) |
| F4 | High | `.github/workflows/checkout.yml:47-48`; `analysis_options.yaml:19`; `test/native/install_test.dart:168` | CI is red at HEAD. (a) `dart analyze` at the root analyses `native/hook/build.dart` without `native`'s dependencies (`uri_does_not_exist` for `code_assets`, `hooks`), so the `package` job fails and the publish dry run never runs. (b) Windows: `wsVersion matches pubspec.yaml` fails on CRLF. Both are fixed in the working tree (`native/**` excluded, `readAsLinesSync`) but not committed. | Commit the two fixes. Add `.gitattributes` with `* text=auto eol=lf` so Windows checkouts stop differing. | Verified (`gh run view 43237370035`) |
| F5 | High | `native/lib/ws_native.dart:16-19`; `native/lib/src/bindings.dart:12`; `README.md:86-93` | A Flutter app that follows the README (`import 'package:ws_native/ws_native.dart'; WsNative.install();` in `main`) no longer builds for the web: `Dart library 'dart:ffi' is not available on this platform`. `ws` advertises six platforms, and most Flutter apps share one `main.dart`. | Make `package:ws_native/ws_native.dart` a conditional export: `src/ws_native_stub.dart` (`isAvailable => false`, `install() => false`) by default, the FFI implementation `if (dart.library.ffi)`. Do not re-export `package:ws/native.dart` types from the stub. Add a `dart compile js` smoke test of an entry point that imports `ws_native`. | Verified (`flutter build web`) |
| F6 | High | `.github/workflows/deploy.yml:5-14` | Pushing tag `2.0.0-dev.1` (the tag D32 requires for the GitHub release) triggers the existing workflow, which publishes `ws` from the repository root immediately: with an empty `nativeAssetHashes`, before the release assets exist, and without `ws_native`. pub.dev versions cannot be replaced. | Replace `deploy.yml` with the release workflow before any 2.x tag is pushed, or disable it now (remove the `push: tags` trigger). Its `workflow_dispatch` trigger is useless anyway: pub.dev accepts OIDC only from tag-triggered runs. | Verified by reading; the workflow is on both `master` and `rewrite` |
| F7 | High | `tool/native_release.dart:17-20`; `mise.toml` `release:hashes`; `tool/ci_prebuilt.sh`; `tool/prebuilt.sh:35` | The release chain passes through a workstation and nothing checks it. (a) `native_release.dart` writes an empty hash map and exits 0 when `checksums.json` is missing. (b) `release:hashes` recomputes `checksums.json` from whatever is in `native/prebuilt/` instead of verifying the file CI produced, so a library rebuilt locally by `native:prebuilt` (same directory) or modified on disk is blessed silently. (c) `ci_prebuilt.sh <run-id>` accepts any run of any commit; nothing ties the binaries to the tagged source. (d) The GitHub release and both `pub publish` calls are manual. | Do the release in one tag-triggered workflow: build, verify (F9), checksums, generate `native_release.dart`, attest, create a draft release with assets, publish `ws`, publish staged `ws_native`, undraft. Make the generator fail on a missing file or on fewer than the expected 12 targets. Make the local task verify rather than regenerate. Record the commit SHA next to the hashes. | Verified (a) by running the tool; (b)-(d) by reading |
| F8 | High (legal) | `LICENSE`; `tool/stage_native.sh:15`; ROADMAP §4.7 last item | The binaries contain 107 crates under MIT, Apache-2.0, ISC, BSD-3-Clause (`subtle`), Zlib (`foldhash`), Unicode-3.0 (`unicode-ident`), CDLA-Permissive-2.0 (`webpki-roots`) and `ring` (Apache-2.0 AND ISC). All of these require the notice to accompany binary redistribution. `ws_native` ships only the MIT `LICENSE` of `ws`; the GitHub release assets ship nothing. No copyleft was found. | Generate `THIRD_PARTY_LICENSES` with `cargo about` in the release workflow. Append it to the staged `LICENSE` separated by a line of 80 hyphens: Flutter's `LicenseRegistry` then shows it in `showLicensePage` with no work for app authors. Upload the same file as a release asset and mention it in `ws:install` output. Add `cargo deny check licenses` to CI with an allow-list. | Verified (`cargo metadata`) |
| F9 | Medium | `.github/workflows/prebuilt.yml:45-59` | Nothing verifies what was built. D9 states the 16 KB alignment is "checked with `llvm-readelf -l` in CI"; no such step exists. Today's artifacts are correct (checked by hand), but a toolchain or `build.rs` change would ship a 4 KB-aligned Android library, a raised glibc floor, a higher `minos`, extra exported symbols or a wrong ABI number unnoticed. | Add a verify step per target: Android `LOAD` alignment `2**14`; Linux max `GLIBC_` and `NEEDED` list; Apple `minos` and install name; export list equals the 13 `ws_native_*` symbols; Windows imports. For host-runnable targets, load the artifact and run the conformance suite against it (with `user_defines: library:`), which also proves the ABI number. | Verified by reading the workflow |
| F10 | Medium | `tool/stage_native.sh:22` (`ws: ^<version>`); `native/lib/ws_native.dart:33-39` | Version skew is unconstrained in one direction. All FFI glue and the tape decoder live in `ws`; the binary lives in `ws_native`. `ws_native 2.0.0` allows any `ws 2.x` (and `^2.0.0-dev.1` allows every later pre-release). When `ws` bumps the ABI, pub resolves new `ws` + old `ws_native`, `WsNative.library` catches the mismatch with `on Object` and returns `null`, and the app silently runs on pure Dart. A tape layout change without an ABI bump would be memory-unsafe. | Publish `ws_native X.Y.Z` with an exact `ws: X.Y.Z` (they are released together from one tag). Distinguish "asset missing" from "ABI mismatch" in `WsNative`: expose the reason and log or assert in debug mode. Add a test that `nativeAbiVersion` equals `ABI_VERSION` in `lib.rs`. | Verified by reading |
| F11 | Medium | `.github/workflows/checkout.yml:53-68`; `native.yml:45-77`; `mise.toml` (`server:build`); `test/support/test_server.dart:29-37` | The server-backed suites (`test/transport/server_suite.dart`, used by the Dart transport test and by `test/native/native_library_test.dart`) are skipped when `server/target/release/ws_bench_server` does not exist. `mise` now builds it (`server:build`), but no workflow does, so CI stays green while running none of those tests. `server/` itself has no CI at all (fmt, clippy, `cargo test`), and `native.yml` path filters do not include `server/**` or `test/support/**`. The binary path is relative to the working directory, so `cd native && dart test` cannot find it either. | In `test-vm` and `conformance`: install Rust, cache `server/`, `cargo build --release`, set `WS_TEST_SERVER` to an absolute path. Make the skip a failure when `CI=true`. Add a `server` job (fmt, clippy `--all-targets`, tests) and the missing path filters. | Verified by reading (workflows vs. `mise.toml`) |
| F12 | Medium | `native/hook/build.dart:37-38, 63-69` | (a) An unknown `source:` value is treated as `auto`: `source: bogus` built and ran with the prebuilt, so a typo in `none` ships the library anyway. (b) The per-OS map that D31 and `NATIVE_DISTRIBUTION.md` describe crashes the hook with `type '_Map<String, dynamic>' is not a subtype of type 'String?'`. (c) `library:` skips the checksum (by design) and is not checked for existence. | Validate against `{auto, none, build, prebuilt}` and throw a message that names the allowed values. Either implement the map or reject it with a clear message. Check that `library:` exists. | Verified (ran all three) |
| F13 | Medium | `lib/src/native/install.dart:67-124`; `bin/install.dart:50-61` | Only `NativeInstallException` is caught. Reproduced unhandled stack traces with exit 255: a missing file on a `file://` mirror (`PathNotFoundException`), and `--target=../../etc` (`Unsupported operation: Illegal path character`). By reading, the same happens for `TimeoutException` from the 60 s stall timer, `HttpException`, `TlsException` other than handshake, and `FileSystemException` (read-only `--out`, replacing a loaded DLL on Windows). `--sha256` with no value is accepted as an empty hash. | Validate `--target` against `^[a-z0-9]+-[a-z0-9]+(-sim)?$` and `--sha256` against 64 hex characters. Catch `IOException` and `TimeoutException` in `main` and print one line. Cap the download size (for example 64 MB), retry transient failures two or three times, use a unique `.part` name, and redact user info and query strings from URLs in messages. | Verified for the two reproduced cases; the rest suspected |
| F14 | Medium | `.github/workflows/*.yml` (all `uses:` lines); `.github/dependabot.yaml` | Supply chain of the workflow that produces binaries for every user: actions are referenced by mutable tags (`dtolnay/rust-toolchain@stable`, `Swatinem/rust-cache@v2`, `browser-actions/setup-chrome@v1`, `actions/*@v4`); no `permissions:` block anywhere; release builds restore a cache; no build provenance. Dependabot covers only `pub` at `/`, so `cargo` (`native/rust`, `server`), `github-actions` and `pub` in `/native` are never updated, and no `cargo audit`/`cargo deny` job exists although the library embeds a TLS stack. | Pin actions to commit SHAs; `permissions: contents: read` by default; no cache on tag builds; `actions/attest-build-provenance` on the libraries; add the three Dependabot ecosystems; add a scheduled `cargo deny check advisories licenses`. | Verified by reading; `cargo audit` is clean today |
| F15 | Medium | `mise.toml:6-7`; workflows (`sdk: stable`, `rust-toolchain@stable`) | Local and CI toolchains differ: mise pins Dart 3.13.7 and Rust 1.99.0; CI ran Dart 3.14.0 and floating stable Rust. A new Rust release can break `clippy -D warnings` with no commit, a new Dart can change `dart format` output, and release binaries are not reproducible from a tag. The declared lower bound `sdk: ^3.10.0` is not tested anywhere. | Add `rust-toolchain.toml` in `native/rust` and `server` (rustup and `dtolnay/rust-toolchain` both honour a pinned version). Pin the Dart version of the format/analyze job to the mise version; keep one `stable` job as an early warning. Add a job with `sdk: 3.10.0` and `dart pub downgrade` that analyses and runs VM tests. | Verified (CI log shows 3.14.0) |
| F16 | Medium | `.github/workflows/native.yml:28, 52` | Windows is never tested on the native side: Rust tests run on Ubuntu only, the conformance suite on Ubuntu and macOS. Windows has its own code paths (`rustls-platform-verifier`/schannel, `ws_native.dll` naming, the hook's cargo call, `DynamicLibrary.open`). The "by path" step also depends on the private layout `native/.dart_tool/hooks_runner/shared/ws_native/build/cargo`. | Add `windows-latest` to both matrices (and `cargo test` on macOS). Build the library once with `cargo build` and pass it through `WS_NATIVE_LIBRARY`, as `mise run test:native` does, instead of searching `.dart_tool`. | Verified by reading |
| F17 | Medium | `tool/build_native.sh:61` (Windows targets) | Both DLLs import `VCRUNTIME140.dll` and the `api-ms-win-crt-*` set. Flutter Windows apps normally bundle the VC++ runtime, but a `dart compile exe` or `dart build cli` server on a clean Windows Server or container (Nano Server) does not have it, and the load fails with a generic "module not found". | Build Windows targets with `-C target-feature=+crt-static` (a `[target.'cfg(all(windows, target_env = "msvc"))']` entry in `native/rust/.cargo/config.toml`), and assert the import list in CI. | Verified (imports via `objdump -p`); the failure on a clean host is suspected |
| F18 | Medium | `.gitignore:22` (`coverage*`); `mise.toml` `coverage` task | `tool/coverage_summary.dart` is ignored by git (`git check-ignore` points at `coverage*`), so it will never be committed, and `mise run coverage` fails on a fresh clone. | Anchor the pattern (`/coverage/`, `/coverage*.info`) in `.gitignore` and `.pubignore`, or rename the tool. | Verified (`git check-ignore -v`) |
| F19 | Medium | `tool/stage_native.sh:13-14`; `native/.pubignore`; `tool/check_pubignore.dart` | The D2 guard does not cover `ws_native`. The staged copy is outside the repository, so only `native/.pubignore` applies, and that file is hand-written (`.dart_tool/`, `build/`, `/rust/target/`, `pubspec.lock`). Anything else untracked under `native/` is published: `.DS_Store`, editor directories, logs, `rust/.cargo/`, stale `prebuilt/<target>` directories from a local build. The checker reads only the root pair of files, and its `# publish: /native/prebuilt/` exception is moot there because `/native/` is ignored as a whole. | Stage from git: `git ls-files native` (or `git archive HEAD native`) plus `prebuilt/`, instead of `rsync` of the working directory. Then let the script fail if `prebuilt/` does not contain exactly the expected targets and a `checksums.json` that matches them. | Verified by reading; today's staged archive happens to be clean |
| F20 | Medium | `lib/src/native/native_release.dart:10` (`nativeReleaseTag = wsVersion`) | Every `ws` release, including a Dart-only patch, needs a GitHub release with all 12 assets under its own tag, or `ws:install` returns 404 for that version. | Let the generator take the native tag as an argument and keep it when the Rust sources did not change, or make the release workflow always attach the assets. Decide which and write it down. | Verified by reading |
| F21 | Low | `native/hook/build.dart` (no log on fallback); `native/lib/ws_native.dart:52` | When no library is bundled (unsupported target such as Android x86 or Linux arm, cargo failure in a git dependency, F10), the only signal is `install() == false`, which the README example ignores. The hook's stderr is not shown on success. | Document checking the return value; add `WsNative.unavailableReason`; ROADMAP §4.7 already lists metrics. | Verified (linux-arm build gives no asset and no message) |
| F22 | Low | `README.md:115-121`; `lib/native.dart:13` | The `compile exe` example calls `WsNativeLibrary.open(path).install()` unguarded. A missing file, wrong architecture, musl (Alpine), F2/F3 or an ABI mismatch after upgrading `ws` without re-running the installer all throw at startup, where pure Dart would have worked. | Show the `try`/fallback form in the README, or add `WsNativeLibrary.tryOpen`. State that the installer must be re-run after every `ws` upgrade. | Verified (missing file and wrong architecture throw `ArgumentError`) |
| F23 | Low | iOS prebuilt | The iOS library imports `fstat` and `clock_gettime` (Rust std), which are "required reason" APIs. Flutter wraps the dylib into a generated `ws_native.framework` that cannot carry a `PrivacyInfo.xcprivacy`, so the app's own manifest must declare the file-timestamp reason. | Add a short "App Store" note to the `ws_native` README (reasons `C617.1` / `0A2A.1`), and re-check imports per release in the verify step (F9). | Imports verified (`nm -u`); the App Store warning is suspected |
| F24 | Low | `native/pubspec.yaml:6` | `repository:` points at the repository root, where the pubspec is `ws`. pub.dev cannot verify the repository for `ws_native`. There are no `issue_tracker`, `topics` or `example/`, which costs pana points. | `repository: https://github.com/plugfox/ws/tree/master/native`, add `issue_tracker`, `topics` and a small `example/`. | Verified by reading; score effect suspected |
| F25 | Low | `.pubignore:63-67`; `pubspec.yaml` `screenshots`; `README.md:12-13, 139-146` | Dead lines after `/native/`. `example.png` is 317 KB of the 392 KB archive and shows 1.x. `test/` is published although it depends on unpublished `server/`. The staged `ws_native` publishes `test/native_transport_test.dart`, which imports `../../test/transport/conformance.dart` and cannot work outside the repository. README links to `docs/` and `benchmark/` resolve on pub.dev only after the merge to `master`, and the "branch `rewrite`" note must go. | Remove the dead lines, ignore `/test/` and `native/test/`, replace or drop the screenshot, use absolute links. | Verified (dry-run listings) |
| F26 | Low | `.github/workflows/checkout.yml:45`; `mise.toml` `fmt:check`, `check` | CI formats `bin lib test tool`; `native/lib`, `native/hook`, `native/test` are formatted only by mise. CI runs neither `actionlint`, `shellcheck`, `check:publish` (so the `ws_native` dry run is never exercised) nor clippy with `--all-targets`. "`mise run check` = what CI checks" is not true in either direction. | One `lint` job that runs `mise run fmt:check analyze check:pubignore check:publish`, so there is a single definition. | Verified by reading |
| F27 | Low | `tool/check_publish.sh:4, 15` | The script fails whenever the tree is dirty (pub exits non-zero on the warning), which is the normal state during development. It also stages `ws_native` without libraries when none are present, so the size and content checks say nothing about the real archive. | Print the warning but tolerate exit code 65 locally; in CI run it on the clean tree and after the libraries are downloaded. | Verified (ws dry run: 1 warning) |
| F28 | Low | `docs/DEVELOPMENT.md:28`; `tool/build_native.sh:13` | "Minimum OS versions match Flutter's: iOS 13.0, macOS 10.15" is out of date: Flutter 3.47 templates use iOS 15.0 and macOS 12.0, Android `minSdk` 24. The lower values are harmless and should stay: the generated iOS framework's `MinimumOSVersion` is 15.0 and the binary's `minos` (13.0) must not exceed it, or App Store validation rejects the bundle. Actual `minos`: macOS arm64 11.0, iOS arm64 simulator 14.0 (Rust floors). | Correct the sentence and add the "must not exceed" rule to the verify step. | Verified (`vtool`, built `Info.plist`) |
| F29 | Low | `actions/checkout@v4`, `actions/upload-artifact@v4` | Node 20 deprecation annotations on every job. | Move to the current majors when pinning SHAs (F14). | Verified (run annotations) |
| F30 | Low | `tool/autobahn.sh:13, 16` | Unpinned image tag and a fixed `sleep 3`; not run in CI. | Pin the image digest, poll the port, add an optional workflow. | Verified by reading |

### Verified to be fine

- `ws` alone: `dart compile exe`, `dart compile js` and `dart compile wasm`
  succeed; `package:ws/ws.dart` pulls in neither `dart:io` nor `dart:ffi`
  on the web. `ws:install` also runs when `ws` is only a transitive
  dependency.
- `ws` + `ws_native`: `dart run`, `dart build cli` (host and cross to
  `linux-x64` from macOS) bundle the prebuilt library with no network and
  no Rust. `dart compile exe` refuses, as D8 documents.
- Flutter 3.47: macOS release builds a universal, signed
  `ws_native.framework` and the launched app reports `install=true`. The
  APK contains `libws_native.so` for `arm64-v8a`, `armeabi-v7a` and
  `x86_64`. The iOS build embeds the framework with a dSYM.
- Android libraries: all `LOAD` segments aligned `2**14` (16 KB). No
  bitcode segment in the Apple libraries. One install name
  (`@rpath/libws_native.dylib`) on every Apple target. 13 exported symbols,
  all `ws_native_*`, on Linux, Apple and Windows (the survey says 11; it
  predates `alloc`/`free`).
- `windows-arm64/ws_native.dll` really is an AArch64 PE file.
- pub validation: 13 MB compressed with all 12 targets, well under the
  100 MiB limit, and the secret scanner raises nothing on the binaries.
- The hook detects a modified library, and re-runs are cached (about 0.9 s
  for a second `dart run`).
- `tool/native_release.dart` output passes `dart format
  --set-exit-if-changed`.
- `example/ws_example.dart` analyses cleanly against the 2.0 API (the root
  `analysis_options.yaml` excludes `example/**`, so CI does not check it).

## Release checklist gaps

What has to exist or be decided before the first 2.x tag:

1. **A release workflow** (F6, F7). Today steps 3 and 4 of
   DEVELOPMENT.md are manual and `deploy.yml` would publish the wrong
   thing. Proposed order inside one tag-triggered workflow: check that the
   tag equals the version in `pubspec.yaml`, `native/pubspec.yaml`,
   `native_release.dart` and both CHANGELOG headings; build 12 targets;
   verify each (F9); checksums; generate `native_release.dart`; create a
   **draft** GitHub release with `<target>-<file>` assets, `checksums.json`
   and the licence notices; `dart pub publish` for `ws`; wait until pub.dev
   serves it; publish the staged `ws_native`; undraft.
2. **Where the hashes live.** The hashes are known only after the build,
   so either the workflow publishes `ws` from a tree that differs from the
   tag by `native_release.dart` (then say so, and attach the generated file
   to the release), or the hashes are committed first and the tagged build
   must reproduce them bit for bit (it will not, today). Pick the first and
   document it.
3. **pub.dev setup.** The first publication of `ws_native` must be manual
   (OIDC cannot create a package). Afterwards enable automated publishing
   for both packages with the same tag pattern, and require a GitHub
   environment with a reviewer for the publish job.
4. **Immutable release assets.** Turn on immutable releases for the
   repository, so assets cannot be replaced after publication. The pinned
   hashes already protect users; this protects availability and the audit
   trail.
5. **Third-party notices** in the `ws_native` archive and on the release
   (F8).
6. **Linux and Windows portability** fixed and asserted in CI (F2, F3,
   F17).
7. **Web stub** in `ws_native` (F5), and the installer argument parser
   (F1), each with a test that executes the README text.
8. **Exact version coupling** between the two packages (F10) and a written
   rule for when `nativeAbiVersion` must be bumped (any change to exported
   signatures, ownership rules or the tape layout).
9. **CI builds the test server** and fails on skipped server suites (F11);
   Windows joins the native matrices (F16).
10. **`pana`** on both packages (staged copy for `ws_native`) and a dry run
    of each from a clean tree with the real libraries present.
11. **Manual smoke tests that no automation covers yet:** a release build
    on a physical Android device (16 KB device or emulator image) and on an
    iPhone, an upload to TestFlight and to a Play internal track, a Windows
    and a Linux Flutter desktop build, and `dart build cli` on Windows.
12. **Cleanup named in D1 and the README:** delete `_lib/`, remove
    `publish_to: none` handling once `ws_native` exists, remove the
    "branch `rewrite`" note, fix the Flutter minimum-version sentence (F28).
13. **Coverage:** no workflow produces or uploads coverage; `mise run
    coverage` exists only locally and its helper is gitignored (F18).

## Future risks to plan for

- **Runner retirement moves the glibc floor.** Without `cargo-zigbuild`
  (F3), the day `ubuntu-22.04` runners disappear the minimum becomes glibc
  2.39 with no code change.
- **musl / Alpine.** There is no musl target. `hostNativeTarget()` reports
  `linux-x64` on Alpine, the installer downloads a glibc library and the
  load fails. Either add `linux-*-musl` targets (and detect musl in the
  installer) or state that Alpine uses pure Dart.
- **Bundled trust roots age.** Android and Linux builds compile in
  `webpki-roots`; Android ignores user-installed and enterprise CAs, which
  `dart:io` honours. A root removal or addition needs a new `ws_native`
  release, so plan a release cadence and a Dependabot rule for that crate,
  and document the behavioural difference.
- **Security fixes in rustls, ring or tungstenite** require a rebuilt
  binary and a release of both packages. Apps that used `ws:install` keep
  the old file until someone re-runs the installer; an ABI-compatible old
  library loads without complaint. Consider embedding the library version
  next to the ABI number and warning when it is older than the package.
- **Hooks API churn.** `hooks` and `code_assets` are at 2.x and Flutter
  pins its own versions; a major bump forces a `ws_native` release that may
  raise the minimum SDK. Keep `ws` free of that dependency, as now, and
  test against Flutter `beta` on a schedule.
- **`dart compile exe` may start supporting hooks**, or pub.dev may host
  per-platform artifacts (D31). Both make the two-package split or the
  13 MB archive unnecessary. D8 already describes merging; also decide now
  whether `ws:install` would be kept.
- **Archive growth.** 13 MB for 12 targets; every user downloads all of
  them on each version. More targets (musl ×2, Linux riscv64, Android
  x86, OHOS) or a larger TLS stack push towards 25–30 MB. The limit is
  100 MiB, but CI `pub get` time and pub cache size are noticed much
  earlier. Keep the size budget from ROADMAP §4.7 as a CI assertion.
- **Code signing expectations.** Windows DLLs are unsigned; enterprise
  WDAC/AppLocker policies and some anti-virus products block unsigned DLLs
  loaded from the pub cache or an app directory. macOS and iOS are covered
  because Flutter re-signs the framework, but a `dart build cli` bundle
  distributed outside the App Store must be signed and notarised by the app
  author, including the dylib. Document both.
- **Apple privacy rules tighten.** If Apple starts enforcing manifests per
  embedded framework, a generated framework without a manifest becomes a
  rejection rather than a warning (F23). Track the Flutter issue for
  resources in code-asset frameworks.
- **Android page size and NDK.** The NDK used in CI is whatever the runner
  image provides. Pin the NDK version, keep the alignment assertion, and
  re-check when Play raises the target API requirements.
- **Reproducibility and provenance.** With floating toolchains and caches,
  the same tag does not rebuild to the same hashes, so third parties cannot
  audit the binaries. Pinned toolchains, `--remap-path-prefix`, no cache on
  release builds and published attestations make the hashes meaningful
  beyond corruption detection.
- **Pre-release semantics.** `^2.0.0-dev.1` matches every later
  pre-release, so `dev` builds with different ABIs will mix in users' lock
  files until exact pinning (F10) is in place.
- **Private mirrors and air-gapped builds.** `--url` with `file://` works
  and should stay tested; add an environment variable for the mirror
  pattern so Dockerfiles do not hard-code it, and document
  `--sha256`/hash pinning for self-built libraries.

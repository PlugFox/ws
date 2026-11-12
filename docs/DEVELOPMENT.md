# Development

Tools and tasks are defined in `mise.toml`.

```bash
mise install        # Dart, Rust (+ rustfmt, clippy), gh, actionlint, shellcheck
mise tasks          # list the tasks
mise run check      # what CI checks: format, analyzers, .pubignore, all tests
```

Things the task list does not show:

- `test:native` runs the shared conformance suite twice. The first run uses
  the library that the `ws_native` hook bundles. The second loads
  `native/rust/target/release` by path through `package:ws/native.dart`
  (`WS_NATIVE_LIBRARY`). A plain `dart test` in the root skips the second
  run.
- `native:prebuilt` builds what this machine can build: Apple targets on
  macOS, plus Android when `ANDROID_NDK_HOME` is set or Android Studio's
  NDK is found. Linux and Windows libraries for a release come from
  `ci:prebuilt`, which runs `.github/workflows/prebuilt.yml` on GitHub,
  waits for it and downloads every target into `native/prebuilt/`.
  - The branch must be pushed first.
  - `workflow_dispatch` only works for workflows that exist on the
    default branch, so until `prebuilt.yml` is merged there, the workflow
    runs on pushes that touch `native/rust/**`. Download such a run with
    `mise run ci:prebuilt <run-id>`.
- Prebuilt libraries target Flutter's minimums: iOS 13.0, macOS 10.15 and
  Android API 21. Linux libraries are linked against glibc 2.17
  with cargo-zigbuild (D40). Android libraries are 16 KB page aligned
  (`rust/build.rs`; `llvm-readelf -l` shows `0x4000`).
- Releasing is one tag (DECISIONS.md D43):
  1. Bump `version` in `pubspec.yaml`, `native/pubspec.yaml` and
     `flutter/pubspec.yaml`, add the same heading to the three CHANGELOGs,
     and run
     `dart run tool/native_release.dart --allow-empty`, which updates
     `wsVersion`. `dart run tool/release_check.dart <version>` checks all
     of it.
  2. Push the tag (`git tag 2.0.0 && git push origin 2.0.0`).
     `release.yml` builds and verifies the 12 libraries, writes the
     hashes, stages `ws_native` with the third-party notices and
     `ws_flutter`, runs the dry runs and creates a **draft** release.
  3. Look at the draft, then approve the `pub.dev` environment. The
     workflow publishes the release and `ws`, then `ws_native` and
     `ws_flutter` (approve those two jobs as well).

  One-time setup before the first tag:
  - Publish `ws_native` and `ws_flutter` once by hand. OIDC cannot create
    a package. Use `release:notices` and `native:stage <dir> <version>
    --release <notices>` on libraries from `ci:prebuilt`, and
    `tool/stage_flutter.sh <dir> <version>`; or download the `ws_native`
    and `ws_flutter` artifacts of a dry run of `release.yml`.
  - On pub.dev, for the three packages: Admin → Automated publishing → GitHub
    Actions, repository `PlugFox/ws`, tag pattern `{{version}}`, require
    environment `pub.dev`.
  - In the repository: create the environment `pub.dev` with required
    reviewers, and turn on immutable releases.

  The release tooling runs on every push to `rewrite` that touches it
  (dry run, nothing is published). `mise run release:hashes` verifies
  `native/prebuilt/checksums.json` before writing the hashes locally;
  it never regenerates them.
- Toolchains: `rust-toolchain.toml` (native/rust, server) and `mise.toml`
  pin the same Rust; `tool/rust_toolchain.sh` fails in CI if they differ.
  `mise run check:deny` runs `cargo deny` (licences, advisories, sources).
- `ws_flutter` (`flutter/`) is a Flutter package: `flutter pub get`,
  `flutter analyze`, `flutter test` there (`mise run test:flutter`). CI
  installs Flutter with `tool/flutter_sdk.sh`, which checks the archive
  against Flutter's release manifest, and tests the lowest supported
  Flutter (3.38.0, Dart 3.10.0) and 3.47.9.
- `ws_native` and `ws_flutter` cannot be published from their
  directories. Pub applies the
  repository's ignore files, which hide it (DECISIONS.md D25), so it is
  staged into a directory outside the repository.

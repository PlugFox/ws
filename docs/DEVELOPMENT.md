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
  Android API 21. Linux builds run on Ubuntu 22.04 runners, so the library
  needs only glibc 2.35. Android libraries are 16 KB page aligned
  (`rust/build.rs`; `llvm-readelf -l` shows `0x4000`).
- Releasing:
  1. `ci:prebuilt` builds the libraries.
  2. `release:hashes` writes `checksums.json` and the hashes into
     `package:ws`.
  3. Create the GitHub release (tag = version) with assets named
     `<target>-<file>`.
  4. `check:publish`, then publish `ws` and the staged `ws_native`
     (`native:stage <dir outside the repo> ^<version>`).

  There is no workflow for steps 3 and 4 yet (ROADMAP §4.7).
- `ws_native` cannot be published from `native/`. Pub applies the
  repository's ignore files, which hide it (DECISIONS.md D25), so it is
  staged into a directory outside the repository.

# ws_native

The Rust transport for [`ws`](https://pub.dev/packages/ws): TLS, WebSocket
framing and JSON parsing run on native threads (tokio, tungstenite, rustls,
simd-json), and messages reach Dart in batches without copies.

```dart
import 'package:ws_native/ws_native.dart';

void main() {
  WsNative.install(); // returns false (pure Dart is used) if unavailable
  runApp(const App());
}
```

The libraries for Android, iOS, macOS, Linux and Windows ship inside this
package. A build hook adds the one for the target to your app, so building
needs no Rust toolchain and no network. Works with Flutter and with
`dart build cli`, `dart run` and `dart test`.

Executables built with `dart compile exe` cannot depend on packages with
build hooks. Use `dart run ws:install` and `package:ws/native.dart` instead
(see the `ws` README).

Opt out, or build from source, per app:

```yaml
hooks:
  user_defines:
    ws_native:
      source: none # none | build | prebuilt
      # library: path/to/libws_native.so  # use this file instead
```

## Licences

The libraries are built from Rust crates under permissive licences (MIT,
Apache-2.0, ISC, BSD-3-Clause, Zlib, Unicode-3.0, CDLA-Permissive-2.0).
Their notices are part of this package's `LICENSE`, so Flutter's
`showLicensePage` lists them with no work on your side.

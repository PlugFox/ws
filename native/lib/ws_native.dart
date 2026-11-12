/// Native (Rust) transport for `package:ws`, with the library bundled by a
/// build hook (Flutter, `dart build cli`, `dart run`, `dart test`).
///
/// ```dart
/// void main() {
///   WsNative.install(); // falls back to pure Dart if the library is missing
///   runApp(...);
/// }
/// ```
///
/// Executables built with `dart compile exe` cannot depend on this package
/// (it has a build hook); they load the library by path with
/// `package:ws/native.dart` instead.
///
/// On the web this library is a stub: [WsNative.install] returns `false`
/// and the browser transport is used, so one `main.dart` serves every
/// platform. `WsNative.library` exists only where `dart:ffi` does.
library;

export 'src/ws_native_ffi.dart'
    if (dart.library.js_interop) 'src/ws_native_stub.dart';

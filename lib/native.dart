/// The Rust transport, loaded from a file (Dart VM and Flutter on Android,
/// iOS, macOS, Windows and Linux; not the web).
///
/// Apps built with Flutter or `dart build cli` normally use the
/// `ws_native` package instead, whose build hook bundles the library.
/// `dart compile exe` refuses to compile a program that depends on a
/// package with a build hook, so for those executables get the library with
/// `dart run ws:install` (again after every `ws` upgrade) and load it:
///
/// ```dart
/// import 'package:ws/native.dart';
///
/// void main() {
///   try {
///     WsNativeLibrary.open('/app/lib/libws_native.so').install();
///     // WebSocketClient instances created from now on use it.
///   } on Object catch (e) {
///     // Missing, another architecture or another release: pure Dart.
///   }
/// }
/// ```
library;

export 'src/native/native_library.dart'
    show NativeSymbols, WsNativeLibrary, nativeAbiVersion;
export 'src/native/native_transport.dart' show NativeTransport;

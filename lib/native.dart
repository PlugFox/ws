/// The Rust transport, loaded from a file (Dart VM and Flutter on Android,
/// iOS, macOS, Windows and Linux; not the web).
///
/// Apps built with Flutter or `dart build cli` normally use the
/// `ws_native` package instead, whose build hook bundles the library. For
/// executables built with `dart compile exe` (which does not run build
/// hooks), get the library with `dart run ws:install` and load it:
///
/// ```dart
/// import 'package:ws/native.dart';
///
/// void main() {
///   WsNativeLibrary.open('/app/lib/libws_native.so').install();
///   // WebSocketClient instances created from now on use it.
/// }
/// ```
library;

export 'src/native/native_library.dart'
    show NativeSymbols, WsNativeLibrary, nativeAbiVersion;
export 'src/native/native_transport.dart' show NativeTransport;

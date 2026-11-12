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
library;

import 'package:ws/native.dart';
import 'package:ws_native/src/bindings.dart';

export 'package:ws/native.dart' show NativeTransport, WsNativeLibrary;

/// Installs the native transport as the default for `WebSocketClient`.
// ignore: avoid_classes_with_only_static_members
abstract final class WsNative {
  static WsNativeLibrary? _library;
  static bool _probed = false;

  /// The bundled library, or `null` when it is missing or incompatible.
  /// Probed once per process (DECISIONS.md D10).
  static WsNativeLibrary? get library {
    if (!_probed) {
      _probed = true;
      try {
        if (bundledAbiVersion() == nativeAbiVersion) {
          _library = WsNativeLibrary.fromSymbols(bundledSymbols());
        }
      } on Object {
        // The code asset is missing (hook source: none, unsupported target).
      }
    }
    return _library;
  }

  /// Whether the bundled library is present and compatible.
  static bool get isAvailable => library != null;

  /// Makes the native transport the default for clients created afterwards
  /// and returns `true`, or returns `false` (and changes nothing) when the
  /// library is not available — clients then use the pure-Dart transport.
  ///
  /// [threads] sets the size of the native runtime's thread pool; it only
  /// applies to the first call in a process.
  static bool install({int threads = 2, int window = 8 << 20}) =>
      library?.install(threads: threads, window: window) ?? false;
}

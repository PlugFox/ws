import 'package:ws/native.dart';
import 'package:ws_native/src/bindings.dart';

export 'package:ws/native.dart' show NativeTransport, WsNativeLibrary;

/// Installs the native transport as the default for `WebSocketClient`.
// ignore: avoid_classes_with_only_static_members
abstract final class WsNative {
  static WsNativeLibrary? _library;
  static bool _probed = false;
  static String? _unavailableReason;

  /// The bundled library, or `null` when it is missing or incompatible
  /// (see [unavailableReason]). Probed once per process (DECISIONS.md D10).
  static WsNativeLibrary? get library {
    if (!_probed) {
      _probed = true;
      try {
        final abi = bundledAbiVersion();
        if (abi == nativeAbiVersion) {
          _library = WsNativeLibrary.fromSymbols(bundledSymbols());
        } else {
          _unavailableReason =
              'the bundled library speaks ABI $abi, package:ws needs '
              '$nativeAbiVersion: use ws and ws_native of the same release';
        }
      } on Object catch (e) {
        // The code asset is missing (hook source: none, unsupported target).
        _unavailableReason = 'no native library is bundled ($e)';
      }
      assert(
        _library != null || _unavailableReason!.startsWith('no native'),
        'ws_native: $_unavailableReason',
      );
    }
    return _library;
  }

  /// Why [library] is `null`, or `null` when it is available.
  static String? get unavailableReason {
    library;
    return _unavailableReason;
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

// The web (and any platform without dart:ffi): no native library; the
// platform transport of package:ws is used.

/// Installs the native transport where `dart:ffi` exists; a no-op here.
// ignore: avoid_classes_with_only_static_members
abstract final class WsNative {
  /// Always `false` without `dart:ffi`.
  static bool get isAvailable => false;

  /// Why the native library is unavailable.
  static String? get unavailableReason => 'dart:ffi is not available';

  /// Returns `false`: clients keep the platform transport.
  static bool install({int threads = 2, int window = 8 << 20}) => false;
}

import 'package:ws/src/transport/transport.dart';
import 'package:ws/src/transport/transport_stub.dart'
    // ignore: uri_does_not_exist
    if (dart.library.js_interop) 'package:ws/src/transport/web/web_transport.dart'
    // ignore: uri_does_not_exist
    if (dart.library.io) 'package:ws/src/transport/dart/dart_transport.dart';

/// The transport used by clients that do not set
/// `WebSocketOptions.transport`.
///
/// Defaults to the platform transport: the browser WebSocket on the web,
/// the pure-Dart RFC 6455 implementation on native platforms. Packages such
/// as `ws_native` install a faster one with [use].
// ignore: avoid_classes_with_only_static_members
abstract final class WebSocketTransports {
  static WebSocketTransport? _current;

  /// The platform's built-in transport.
  static final WebSocketTransport platform = createPlatformTransport();

  /// The transport new clients use.
  static WebSocketTransport get current => _current ?? platform;

  /// Makes [transport] the default for clients created afterwards; `null`
  /// restores the platform transport.
  // ignore: use_setters_to_change_properties
  static void use(WebSocketTransport? transport) => _current = transport;
}

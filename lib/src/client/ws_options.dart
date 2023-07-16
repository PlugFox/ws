import 'dart:io' as io;

import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_options_vm.dart'
    // ignore: uri_does_not_exist
    if (dart.library.html) 'package:ws/src/client/ws_options_js.dart';

/// Backoff strategy for reconnecting.
/// Tweaks for reconnect backoff algorithm (min delay, max delay)
typedef ConnectionRetryInterval = ({Duration min, Duration max});

/// {@template ws_options}
/// Web socket platform dependent options.
///
/// The [connectionRetryInterval] argument is specifying the
/// backoff strategy for reconnecting.
/// Tweaks for reconnect backoff algorithm (min delay, max delay)
/// If not specified, the reconnecting will be disabled.
///
/// The [protocols] argument is specifying the subprotocols the
/// client is willing to speak.
///
/// {@endtemplate}
@immutable
abstract base class WebSocketOptions {
  /// {@macro ws_options}
  WebSocketOptions({
    this.connectionRetryInterval,
    Iterable<String>? protocols,
  }) : protocols = protocols?.toSet();

  /// {@macro ws_options_common}
  factory WebSocketOptions.common({
    ConnectionRetryInterval? connectionRetryInterval,
    Iterable<String>? protocols,
  }) = WebSocketOptions$Common;

  /// {@template ws_options_vm}
  /// Options for VM (Mobile, Desktop, Server, Console) platform.
  /// Do not use this class at the web platform.
  ///
  /// The [connectionRetryInterval] argument is specifying the
  /// backoff strategy for reconnecting.
  /// Tweaks for reconnect backoff algorithm (min delay, max delay)
  /// If not specified, the reconnecting will be disabled.
  ///
  /// The [protocols] argument is specifying the subprotocols the
  /// client is willing to speak.
  ///
  /// The [headers] argument is specifying additional HTTP headers for
  /// setting up the connection. This would typically be the `Origin`
  /// header and potentially cookies. The keys of the map are the header
  /// fields and the values are either String or List<String>.
  ///
  /// If [headers] is provided, there are a number of headers
  /// which are controlled by the WebSocket connection process. These
  /// headers are:
  ///
  ///   - `connection`
  ///   - `sec-websocket-key`
  ///   - `sec-websocket-protocol`
  ///   - `sec-websocket-version`
  ///   - `upgrade`
  ///
  /// If any of these are passed in the `headers` map they will be ignored.
  ///
  /// If [compression] is provided, the `WebSocket` created will be configured
  /// to negotiate with the specified `CompressionOptions`. If none is specified
  /// then the `WebSocket` will be created
  /// with the default `CompressionOptions`.
  ///
  /// If [customClient] is provided, the `WebSocket` created will be configured.
  ///
  /// If [userAgent] is provided, the `WebSocket` created will be configured.
  /// {@endtemplate}
  factory WebSocketOptions.vm({
    ConnectionRetryInterval? connectionRetryInterval,
    Iterable<String>? protocols,
    Map<String, dynamic>? headers,
    io.CompressionOptions? compression,
    io.HttpClient? customClient,
    String? userAgent,
  }) =>
      $vmOptions(
        connectionRetryInterval: connectionRetryInterval,
        protocols: protocols,
        headers: headers,
        compression: compression,
        customClient: customClient,
        userAgent: userAgent,
      );

  /// {@template ws_options_js}
  /// Options for JS (Browser) platform.
  /// Do not use this class at the VM platform.
  ///
  /// The [connectionRetryInterval] argument is specifying the
  /// backoff strategy for reconnecting.
  /// Tweaks for reconnect backoff algorithm (min delay, max delay)
  /// If not specified, the reconnecting will be disabled.
  ///
  /// The [protocols] argument is specifying the subprotocols the
  /// client is willing to speak.
  /// {@endtemplate}
  @internal
  factory WebSocketOptions.js({
    ConnectionRetryInterval? connectionRetryInterval,
    Iterable<String>? protocols,
  }) =>
      $jsOptions(
        connectionRetryInterval: connectionRetryInterval,
        protocols: protocols,
      );

  /// Construct options for VM or JS platform depending on the current platform.
  ///
  /// {@macro ws_options}
  factory WebSocketOptions.selector({
    required WebSocketOptions Function() vm,
    required WebSocketOptions Function() js,
  }) =>
      $selectorOptions(
        vm: vm,
        js: js,
      );

  /// Backoff strategy for reconnecting.
  /// Tweaks for reconnect backoff algorithm (min delay, max delay)
  /// If not specified, the reconnecting will be disabled.
  @nonVirtual
  final ConnectionRetryInterval? connectionRetryInterval;

  /// Web Socket protocols.
  /// If not specified, the protocols will not be used.
  @nonVirtual
  final Set<String>? protocols;
}

/// {@template ws_options_common}
/// Options for all platforms.
///
/// The [connectionRetryInterval] argument is specifying the
/// backoff strategy for reconnecting.
/// Tweaks for reconnect backoff algorithm (min delay, max delay)
/// If not specified, the reconnecting will be disabled.
///
/// The [protocols] argument is specifying the subprotocols the
/// client is willing to speak.
///
/// {@endtemplate}
@internal
final class WebSocketOptions$Common extends WebSocketOptions {
  /// {@macro ws_options_common}
  WebSocketOptions$Common({
    super.connectionRetryInterval,
    super.protocols,
  });
}

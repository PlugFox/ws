import 'dart:io';

import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_options.dart';

/// {@nodoc}
@internal
WebSocketOptions $vmOptions({
  ConnectionRetryInterval? connectionRetryInterval,
  Iterable<String>? protocols,
  Map<String, Object?>? headers,
  Object? /*CompressionOptions*/ compression,
  Object? /*HttpClient*/ customClient,
  String? userAgent,
}) =>
    $WebSocketOptions$VM(
      connectionRetryInterval: connectionRetryInterval,
      protocols: protocols,
      headers: headers,
      compression: switch (compression) {
        CompressionOptions compression => compression,
        _ => null,
      },
      customClient: switch (customClient) {
        HttpClient customClient => customClient,
        _ => null,
      },
      userAgent: userAgent,
    );

/// {@nodoc}
@internal
WebSocketOptions $jsOptions({
  ConnectionRetryInterval? connectionRetryInterval,
  Iterable<String>? protocols,
}) {
  assert(false, 'This method should not be called at the VM platform.');
  return $WebSocketOptions$VM(
    connectionRetryInterval: connectionRetryInterval,
    protocols: protocols,
  );
}

/// {@nodoc}
@internal
WebSocketOptions $selectorOptions({
  required WebSocketOptions Function() vm,
  required WebSocketOptions Function() js,
}) =>
    vm();

/// {@nodoc}
@internal
final class $WebSocketOptions$VM extends WebSocketOptions {
  /// {@nodoc}
  $WebSocketOptions$VM({
    super.connectionRetryInterval,
    super.protocols,
    this.headers,
    this.compression,
    this.customClient,
    this.userAgent,
  });

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
  /// If any of these are passed in the [headers] map they will be ignored.
  final Map<String, dynamic>? headers;

  /// If [compression] is provided, the [WebSocket] created will be configured
  /// to negotiate with the specified [CompressionOptions]. If none is specified
  /// then the [WebSocket] will be created with
  /// the default [CompressionOptions].
  final CompressionOptions? compression;

  /// The [customClient] to be used instead of the default [HttpClient].
  final HttpClient? customClient;

  /// User agent used for WebSocket connections.
  final String? userAgent;
}

// Ignore web related imports at the GitHub Actions coverage.
// coverage:ignore-file
import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_interceptor.dart';
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
  Duration? timeout,
  FutureOr<void> Function(IWebSocketClient)? afterConnect,
  Iterable<WSInterceptor>? interceptors,
}) {
  assert(false, 'This method should not be called at the JS platform.');
  return $WebSocketOptions$JS(
    connectionRetryInterval: connectionRetryInterval,
    protocols: protocols,
    timeout: timeout,
    afterConnect: afterConnect,
    interceptors: interceptors,
  );
}

/// {@nodoc}
@internal
WebSocketOptions $jsOptions({
  ConnectionRetryInterval? connectionRetryInterval,
  Iterable<String>? protocols,
  Duration? timeout,
  FutureOr<void> Function(IWebSocketClient)? afterConnect,
  Iterable<WSInterceptor>? interceptors,
  bool? useBlobForBinary,
}) =>
    $WebSocketOptions$JS(
      connectionRetryInterval: connectionRetryInterval,
      protocols: protocols,
      timeout: timeout,
      useBlobForBinary: useBlobForBinary,
      afterConnect: afterConnect,
      interceptors: interceptors,
    );

/// {@nodoc}
@internal
WebSocketOptions $selectorOptions({
  required WebSocketOptions Function() vm,
  required WebSocketOptions Function() js,
}) =>
    js();

/// {@nodoc}
final class $WebSocketOptions$JS extends WebSocketOptions {
  /// {@macro ws_options_js}
  $WebSocketOptions$JS({
    super.connectionRetryInterval,
    super.protocols,
    super.timeout,
    super.afterConnect,
    super.interceptors,
    bool? useBlobForBinary,
  }) : useBlobForBinary = useBlobForBinary ?? false;

  /// {@nodoc}
  final bool useBlobForBinary;
}

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
}) {
  assert(false, 'This method should not be called at the JS platform.');
  return $WebSocketOptions$JS(
    connectionRetryInterval: connectionRetryInterval,
    protocols: protocols,
  );
}

/// {@nodoc}
@internal
WebSocketOptions $jsOptions({
  ConnectionRetryInterval? connectionRetryInterval,
  Iterable<String>? protocols,
}) =>
    $WebSocketOptions$JS(
      connectionRetryInterval: connectionRetryInterval,
      protocols: protocols,
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
  });
}

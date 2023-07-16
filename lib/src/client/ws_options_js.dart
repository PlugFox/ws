import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_options.dart';

/// {@nodoc}
@internal
WebSocketOptions $vmOptions({
  ConnectionRetryInterval? connectionRetryInterval,
  Iterable<String>? protocols,
  // ignore: type_annotate_public_apis, inference_failure_on_untyped_parameter
  headers,
  // ignore: type_annotate_public_apis, inference_failure_on_untyped_parameter
  compression,
  // ignore: type_annotate_public_apis, inference_failure_on_untyped_parameter
  customClient,
  // ignore: type_annotate_public_apis, inference_failure_on_untyped_parameter
  userAgent,
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

import 'package:meta/meta.dart';
import 'package:ws/src/platform/platform.i.dart';

/// Get the platform WebSocket transport client for the current environment.
/// {@nodoc}
@internal
IWebSocketPlatformTransport $getWebSocketTransport({
  required final void Function(Object data) onReceived,
  required final void Function(Object data) onSent,
  required final void Function(Object error, StackTrace stackTrace) onError,
  required final void Function(String url) onConnected,
  required final void Function(int? code, String? reason) onDisconnected,
}) =>
    throw UnsupportedError(
        'Cannot create a WebSocket without dart:html or dart:io.');

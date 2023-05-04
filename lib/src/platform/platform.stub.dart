import 'package:meta/meta.dart';
import 'package:ws/src/platform/platform.i.dart';

/// Get the platform WebSocket transport client for the current environment.
/// {@nodoc}
@internal
IWebSocketPlatformTransport $getWebSocketTransport(String url) =>
    throw UnsupportedError(
        'Cannot create a WebSocket without dart:html or dart:io.');

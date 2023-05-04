import 'package:meta/meta.dart';
import 'package:ws/src/platform/platform.i.dart';

@internal
IWebSocketPlatformTransport $getWebSocketTransport() => throw UnsupportedError(
    'Cannot create a WebSocket without dart:html or dart:io.');

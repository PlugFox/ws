import 'package:meta/meta.dart';
import 'package:ws/src/platform/platform.i.dart';

/// Base class for platform WebSocket transport.
/// {@nodoc}
abstract base class WebSocketPlatformTransport$Base
    implements IWebSocketPlatformTransport {
  /// {@nodoc}
  WebSocketPlatformTransport$Base({
    required this.onReceived,
    required this.onSent,
    required this.onConnected,
    required this.onDisconnected,
    required this.onError,
  });

  @override
  @mustCallSuper
  void close([int? code = 1000, String? reason = 'Normal Closure']) {}

  @override
  @protected
  @nonVirtual
  final void Function(Object data) onReceived;

  @override
  @protected
  @nonVirtual
  final void Function(Object data) onSent;

  @override
  @protected
  @nonVirtual
  final void Function(String url) onConnected;

  @override
  @protected
  @nonVirtual
  final void Function(int? code, String? reason) onDisconnected;

  @override
  @protected
  @nonVirtual
  final void Function(Object error, StackTrace stackTrace) onError;
}

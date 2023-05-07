import 'dart:async';
import 'dart:developer';

import 'package:meta/meta.dart';
import 'package:ws/src/client/ws_client.i.dart';
import 'package:ws/src/model/websocket_exception.dart';
import 'package:ws/src/platform/platform.dart';
import 'package:ws/src/util/constants.dart';

/// {@template ws_client}
/// WebSocket client.
/// {@endtemplate}
/// {@category Client}
class WebSocketClient implements IWebSocketClient {
  /// {@macro ws_client}
  WebSocketClient() : _controller = StreamController<Object>.broadcast() {
    _transport = $getWebSocketTransport(
      onReceived: _$onReceived,
      onSent: _$onSent,
      onError: _$onError,
      onConnected: _$onConnected,
      onDisconnected: _$onDisconnected,
    );
  }

  /// {@macro ws_client}
  factory WebSocketClient.connect(String url) =>
      WebSocketClient()..connect(url);

  /// {@nodoc}
  late final IWebSocketPlatformTransport _transport;

  /// Output stream of data from native WebSocket client.
  /// {@nodoc}
  final StreamController<Object> _controller;

  @override
  @nonVirtual
  Stream<Object> get stream => _controller.stream;

  @override
  @mustCallSuper
  FutureOr<void> add(Object data) => _transport.add(data);

  @override
  @mustCallSuper
  Future<void> connect(String url) => _transport.connect(url);

  @override
  @mustCallSuper
  void disconnect([int? code, String? reason]) =>
      _transport.disconnect(code, reason);

  @override
  @mustCallSuper
  void close([int? code = 1000, String? reason = 'Normal Closure']) {
    _transport.close(code, reason);
    _controller.close().ignore();
  }

  /// On message received from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  void _$onReceived(Object data) {
    if (_controller.isClosed) {
      assert(false, 'Cannot receive data to a closed stream controller.');
    } else {
      _controller.add(data);
    }
  }

  /// On message sent.
  /// {@nodoc}
  @nonVirtual
  void _$onSent(Object data) {}

  /// Receive error from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  @pragma('vm:invisible')
  void _$onError(Object error, StackTrace stackTrace) {
    debugger(when: $kDebugWS);
    if (_controller.isClosed) {
      assert(false, 'Cannot receive error to a closed stream controller.');
    } else {
      switch (error) {
        case WSException e:
          error = e;
          break;
        case String e:
          error = WSUnknownException(e);
          break;
        case StateError e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.message);
          break;
        case UnsupportedError e:
          debugger(when: $kDebugWS);
          error = WSUnsupportedException(e.message ?? 'Unsupported exception.');
          break;
        case Exception e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.toString());
          break;
        case Error e:
          debugger(when: $kDebugWS);
          error = WSUnknownException(e.toString());
          break;
        case Object:
          debugger(when: $kDebugWS);
          error = WSUnknownException(error.toString());
          break;
      }
      _controller.addError(error, stackTrace);
    }
  }

  /// On connection established.
  /// {@nodoc}
  @nonVirtual
  void _$onConnected(String url) {
    assert(url.isNotEmpty, 'URL cannot be empty.');
    assert(!_controller.isClosed, 'Controller already closed.');
  }

  /// On connection closed.
  /// {@nodoc}
  @nonVirtual
  void _$onDisconnected(int? code, String? reason) {}
}

import 'dart:async';
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/websocket_exception.dart';
import 'package:ws/src/client/ws_client_base.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
IWebSocketClient $platformWebSocketClient(Duration reconnectTimeout) =>
    WebSocketClient$IO(reconnectTimeout: reconnectTimeout);

/// {@nodoc}
@internal
final class WebSocketClient$IO extends WebSocketClientBase {
  /// {@nodoc}
  WebSocketClient$IO({super.reconnectTimeout});

  /// Native WebSocket client.
  /// {@nodoc}
  io.WebSocket? _client;

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
  /// {@nodoc}
  StreamSubscription<Object?>? _dataBindSubscription;

  @override
  WebSocketReadyState get readyState {
    final code = _client?.readyState;
    assert(code == null || code >= 0 && code <= 3, 'Invalid readyState code.');
    return code == null
        ? WebSocketReadyState.closed
        : WebSocketReadyState.fromCode(code);
  }

  @override
  FutureOr<void> add(Object data) {
    assert(_client != null, 'WebSocket client is not connected.');
    try {
      switch (data) {
        case String text:
          _client?.addUtf8Text(text.codeUnits);
        case TypedData td:
          _client?.add(td.buffer.asInt8List());
        case ByteBuffer bb:
          _client?.add(bb.asInt8List());
        case List<int> bytes:
          _client?.add(bytes);
        default:
          throw ArgumentError.value(data, 'data', 'Invalid data type.');
      }
    } on Object catch (error, stackTrace) {
      warning(error, stackTrace, 'WebSocketClient\$IO.add: $error');
      onError(error, stackTrace);
      rethrow;
    }
  }

  @override
  FutureOr<void> connect(String url) async {
    try {
      super.connect(url);
      disconnect(1001, 'RECONNECTING');
      _client = await io.WebSocket.connect(url);
      _dataBindSubscription = _client?.listen(
        onReceivedData,
        onError: onError,
        onDone: () => disconnect(1000, 'SUBSCRIPTION_CLOSED'),
        cancelOnError: false,
      );
      /* if (!readyState.isOpen) {
      disconnect(1001, 'IS_NOT_OPEN_AFTER_CONNECT');
      assert(
        false,
        'Invalid readyState code after connect: $readyState',
      );
    } */
    } on io.SocketException catch (error, stackTrace) {
      // That error is only for I/O environment.
      final exception = WSSocketException(error.message);
      disconnect(1006, 'CONNECTION_FAILED');
      onError(exception, stackTrace);
      Error.throwWithStackTrace(exception, stackTrace);
    } on io.HttpException catch (error, stackTrace) {
      // That error is only for I/O environment.
      final exception = WSHttpException(error.message);
      disconnect(1006, 'CONNECTION_FAILED');
      onError(exception, stackTrace);
      Error.throwWithStackTrace(exception, stackTrace);
    } on io.WebSocketException catch (error, stackTrace) {
      disconnect(1006, 'CONNECTION_FAILED');
      onError(error, stackTrace);
      rethrow;
    } on Object catch (error, stackTrace) {
      disconnect(1006, 'CONNECTION_FAILED');
      onError(error, stackTrace);
      rethrow;
    }
  }

  @override
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    _dataBindSubscription?.cancel().ignore();
    Future<void>.sync(() => _client?.close(code, reason)).ignore();
    _client = null;
  }

  @override
  void close([int? code = 1000, String? reason = 'NORMAL_CLOSURE']) {
    disconnect(code, reason);
    _client = null;
  }
}

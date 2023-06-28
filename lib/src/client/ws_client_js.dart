import 'dart:async';
import 'dart:html' as html
    show WebSocket, Blob, Event, MessageEvent, CloseEvent, FileReader;
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
    WebSocketClient$JS(reconnectTimeout: reconnectTimeout);

/// {@nodoc}
@internal
final class WebSocketClient$JS extends WebSocketClientBase {
  /// {@nodoc}
  WebSocketClient$JS({super.reconnectTimeout});

  /// Native WebSocket client.
  /// {@nodoc}
  html.WebSocket? _client;

  late final _BlobReader _blobReader = _BlobReader();

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
  /// {@nodoc}
  StreamSubscription<Object?>? _dataBindSubscription;

  /// Binding to error from native WebSocket client.
  /// {@nodoc}
  StreamSubscription<html.Event>? _errorBindSubscription;

  /// Binding to close event from native WebSocket client.
  /// {@nodoc}
  StreamSubscription<html.CloseEvent>? _closeBindSubscription;

  /// Ready state of the WebSocket client.
  /// {@nodoc}
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
    super.add(data);
    final client = _client;
    if (client == null) {
      throw const WSClientClosed('WebSocket client is not connected.');
    }
    try {
      switch (data) {
        case String text:
          client.sendString(text);
        case TypedData td:
          client.sendTypedData(td);
        case html.Blob blob:
          client.sendBlob(blob);
        case ByteBuffer bb:
          client.sendByteBuffer(bb);
        case List<int> bytes:
          client.send(bytes);
        default:
          throw ArgumentError.value(data, 'data', 'Invalid data type.');
      }
    } on Object catch (error, stackTrace) {
      warning(error, stackTrace, 'WebSocketClient\$JS.add: $error');
      onError(error, stackTrace);
      rethrow;
    }
  }

  @override
  FutureOr<void> connect(String url) async {
    try {
      super.connect(url);
      if (_client != null) disconnect(1001, 'RECONNECTING');
      _client = html.WebSocket(url);
      await _client?.onOpen.first;
      _errorBindSubscription = _client?.onError.listen(
        (event) => onError(event, StackTrace.current),
        onError: onError,
        onDone: disconnect,
        cancelOnError: false,
      );
      _dataBindSubscription = _client?.onMessage
          .map<Object?>((event) => event.data)
          .asyncMap<Object?>((data) => switch (data) {
                String text => text,
                TypedData td => td.buffer.asInt8List(),
                html.Blob blob => _blobReader.read(blob),
                ByteBuffer bb => bb.asInt8List(),
                List<int> bytes => bytes,
                _ => data,
              })
          .listen(
            onReceivedData,
            onError: onError,
            onDone: disconnect,
            cancelOnError: false,
          );
      _closeBindSubscription = _client?.onClose.listen(
        (event) => disconnect(event.code, event.reason),
        onError: onError,
        cancelOnError: false,
      );
      /* if (!readyState.isOpen) {
        disconnect(1001, 'IS_NOT_OPEN_AFTER_CONNECT');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      } */
      super.onConnected(url);
    } on Object catch (error, stackTrace) {
      disconnect(1006, 'CONNECTION_FAILED');
      onError(error, stackTrace);
      rethrow;
    }
  }

  @override
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    await super.disconnect(code, reason);
    _errorBindSubscription?.cancel().ignore();
    _closeBindSubscription?.cancel().ignore();
    _dataBindSubscription?.cancel().ignore();
    Future<void>.sync(() => _client?.close(code, reason)).ignore();
    _client = null;
    super.onDisconnected(code, reason);
  }

  @override
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    await super.close(code, reason);
    _client = null;
  }
}

class _BlobReader {
  _BlobReader();

  FutureOr<Object> read(html.Blob blob) async {
    final completer = Completer<Object>();

    void complete(Object data) {
      if (completer.isCompleted) return;
      completer.complete(data);
    }

    void completeError(Object error, [StackTrace? stackTrace]) {
      if (completer.isCompleted) return;
      completer.completeError(error, stackTrace);
    }

    late final html.FileReader reader;
    reader = html.FileReader()
      ..onLoadEnd.listen((_) {
        final result = reader.result;
        switch (result) {
          case String text:
            complete(text);
            break;
          case Uint8List bytes:
            complete(bytes);
            break;
          case ByteBuffer bb:
            complete(bb.asUint8List());
            break;
          default:
            completeError('Unexpected result type: ${result.runtimeType}');
        }
      })
      ..onError.listen(completeError)
      ..readAsArrayBuffer(blob);
    return completer.future;
  }
}

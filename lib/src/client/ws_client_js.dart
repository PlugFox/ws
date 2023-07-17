import 'dart:async';
import 'dart:convert';
import 'dart:html' as html show WebSocket, Blob, Event, CloseEvent, FileReader;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/websocket_exception.dart';
import 'package:ws/src/client/ws_client_base.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_options.dart';
import 'package:ws/src/client/ws_options_js.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
IWebSocketClient $platformWebSocketClient(WebSocketOptions? options) =>
    switch (options) {
      $WebSocketOptions$JS options => WebSocketClient$JS(
          protocols: options.protocols,
          options: options,
        ),
      _ => WebSocketClient$JS(
          protocols: options?.protocols,
        ),
    };

/// {@nodoc}
@internal
final class WebSocketClient$JS extends WebSocketClientBase {
  /// {@nodoc}
  WebSocketClient$JS({super.protocols, $WebSocketOptions$JS? options})
      : _options = options;

  /// {@nodoc}
  final $WebSocketOptions$JS? _options;

  /// Native WebSocket client.
  /// {@nodoc}
  html.WebSocket? _client;

  /// Blob codec for `Blob <-> List<int>` conversion.
  /// {@nodoc}
  late final _BlobCodec _blobCodec = _BlobCodec();

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
      throw const WSClientClosedException(
          message: 'WebSocket client is not connected.');
    }
    try {
      switch (data) {
        case String text:
          client.sendString(text);
        case html.Blob blob:
          client.sendBlob(blob);
        default:
          if (_options?.useBlobForBinary == true) {
            client.sendBlob(_blobCodec.write(data));
          } else {
            switch (data) {
              case TypedData td:
                client.sendTypedData(td);
              case ByteBuffer bb:
                client.sendByteBuffer(bb);
              case List<int> bytes:
                client.sendTypedData(Uint8List.fromList(bytes));
              default:
                throw ArgumentError.value(data, 'data', 'Invalid data type.');
            }
          }
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
      if (_client != null) await disconnect(1001, 'RECONNECTING');
      super.connect(url);
      final client = _client = html.WebSocket(url, protocols);
      final completer = Completer<void>();
      client.onOpen.first.whenComplete(() {
        if (completer.isCompleted) return;
        completer.complete();
      }).ignore();
      _errorBindSubscription = client.onError.listen(
        (event) {
          if (completer.isCompleted) {
            onError(event, StackTrace.current);
          } else {
            completer.completeError(const WSNotConnectedException());
          }
        },
        onError: onError,
        onDone: disconnect,
        cancelOnError: false,
      );
      _dataBindSubscription = client.onMessage
          .map<Object?>((event) => event.data)
          .asyncMap<Object?>((data) => switch (data) {
                String text => text, // coverage:ignore-line
                html.Blob blob => _blobCodec.read(blob), // coverage:ignore-line
                TypedData td => td.buffer.asInt8List(), // coverage:ignore-line
                ByteBuffer bb => bb.asInt8List(), // coverage:ignore-line
                List<int> bytes => bytes, // coverage:ignore-line
                _ => data, // coverage:ignore-line
              })
          .listen(
            onReceivedData,
            onError: onError,
            onDone: disconnect,
            cancelOnError: false,
          );
      _closeBindSubscription = client.onClose.listen(
        (event) => disconnect(event.code, event.reason),
        onError: onError,
        cancelOnError: false,
      );
      await completer.future;
      if (!readyState.isOpen) {
        disconnect(1001, 'IS_NOT_OPEN_AFTER_CONNECT');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      }
      super.onConnected(url);
    } on Object catch (error, stackTrace) {
      onError(error, stackTrace);
      Future<void>.sync(() => disconnect(1006, 'CONNECTION_FAILED')).ignore();
      rethrow;
    }
  }

  @override
  FutureOr<void> disconnect(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    final client = _client;
    await super.disconnect(code, reason);
    _errorBindSubscription?.cancel().ignore();
    _closeBindSubscription?.cancel().ignore();
    _dataBindSubscription?.cancel().ignore();
    _errorBindSubscription = null;
    _closeBindSubscription = null;
    _dataBindSubscription = null;
    if (client != null) {
      try {
        client.close(code, reason);
      } on Object {/* ignore */}
      _client = null;
    }
    super.onDisconnected(code, reason);
  }

  @override
  FutureOr<void> close(
      [int? code = 1000, String? reason = 'NORMAL_CLOSURE']) async {
    await super.close(code, reason);
    _client = null;
  }
}

/// {@nodoc}
final class _BlobCodec {
  /// {@nodoc}
  _BlobCodec();

  /// {@nodoc}
  @internal
  html.Blob write(Object data) {
    switch (data) {
      case String text:
        return html.Blob([Uint8List.fromList(utf8.encode(text))]);
      case TypedData td:
        return html.Blob([td.buffer.asUint8List()]);
      case ByteBuffer bb:
        return html.Blob([bb.asUint8List()]);
      case List<int> bytes:
        return html.Blob([Uint8List.fromList(bytes)]);
      default:
        throw ArgumentError.value(data, 'data', 'Invalid data type.');
    }
  }

  /// {@nodoc}
  @internal
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

// Ignore web related imports at the GitHub Actions coverage.
// coverage:ignore-file
import 'dart:async';
import 'dart:convert';
import 'dart:js_interop' as js;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:web/web.dart' as web
    show Blob, CloseEvent, Event, WebSocket, WebSocketEvents;
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/websocket_exception.dart';
import 'package:ws/src/client/ws_client_base.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_options.dart';
import 'package:ws/src/client/ws_options_js.dart';
import 'package:ws/src/util/logger.dart';

/// Platform related callback to create a WebSocket client.
@internal
IWebSocketClient $platformWebSocketClient(WebSocketOptions? options) =>
    switch (options) {
      $WebSocketOptions$JS options => WebSocketClient$JS(
          interceptors: options.interceptors,
          protocols: options.protocols,
          options: options,
        ),
      _ => WebSocketClient$JS(
          interceptors: options?.interceptors,
          protocols: options?.protocols,
          options: $WebSocketOptions$JS(
            protocols: options?.protocols,
            connectionRetryInterval: options?.connectionRetryInterval,
            timeout: options?.timeout,
            afterConnect: options?.afterConnect,
            interceptors: options?.interceptors,
          ),
        ),
    };

@internal
final class WebSocketClient$JS extends WebSocketClientBase {
  WebSocketClient$JS({
    super.interceptors,
    super.protocols,
    $WebSocketOptions$JS? options,
  }) : _options = options;

  final $WebSocketOptions$JS? _options;

  /// Native WebSocket client.
  web.WebSocket? _client;

  /// Blob codec for `Blob <-> List<int>` conversion.
  late final _BlobCodec _blobCodec = _BlobCodec();

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
  StreamSubscription<Object?>? _dataBindSubscription;

  /// Binding to error from native WebSocket client.
  StreamSubscription<web.Event>? _errorBindSubscription;

  /// Binding to close event from native WebSocket client.
  StreamSubscription<web.CloseEvent>? _closeBindSubscription;

  /// Ready state of the WebSocket client.
  @override
  WebSocketReadyState get readyState {
    final code = _client?.readyState;
    assert(code == null || code >= 0 && code <= 3, 'Invalid readyState code.');
    return code == null
        ? WebSocketReadyState.closed
        : WebSocketReadyState.fromCode(code);
  }

  @override
  void push(Object data) {
    final client = _client;
    if (client == null || client.readyState != web.WebSocket.OPEN) {
      throw const WSClientClosedException(
          message: 'WebSocket client is not connected.');
    }
    try {
      switch (data) {
        case String text:
          client.send(text.toJS);
        case web.Blob blob:
          client.send(blob);
        default:
          if (_options?.useBlobForBinary == true) {
            client.send(_blobCodec.write(data));
          } else {
            switch (data) {
              case TypedData td:
                client.send(Uint8List.view(
                  td.buffer,
                  td.offsetInBytes,
                  td.lengthInBytes,
                ).toJS);
              case ByteBuffer bb:
                client.send(bb.toJS);
              case List<int> bytes:
                client.send(Uint8List.fromList(bytes).toJS);
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
      final client = _client = web.WebSocket(
        url,
        protocols?.map((e) => e.toJS).toList().toJS ?? <js.JSString>[].toJS,
      );
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
                String text => text,
                web.Blob blob => _blobCodec.read(blob),
                TypedData td => Uint8List.view(
                    td.buffer,
                    td.offsetInBytes,
                    td.lengthInBytes,
                  ),
                ByteBuffer bb => bb.asInt8List(),
                List<int> bytes => bytes,
                _ => data,
              })
          .listen(
            super.onReceivedData,
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
      // coverage:ignore-start
      if (!readyState.isOpen) {
        disconnect(1001, 'IS_NOT_OPEN_AFTER_CONNECT');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      }
      // coverage:ignore-end
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
        if (code != null && reason != null) {
          client.close(code, reason);
        } else {
          client.close();
        }
      } on Object {/* ignore */} // coverage:ignore-line
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

final class _BlobCodec {
  _BlobCodec();

  @internal
  web.Blob write(Object data) {
    switch (data) {
      case String text:
        return web.Blob([Uint8List.fromList(utf8.encode(text)).toJS].toJS);
      case TypedData td:
        return web.Blob([
          Uint8List.view(
            td.buffer,
            td.offsetInBytes,
            td.lengthInBytes,
          ).toJS
        ].toJS);
      case ByteBuffer bb:
        return web.Blob([bb.asUint8List().toJS].toJS);
      case List<int> bytes:
        return web.Blob([Uint8List.fromList(bytes).toJS].toJS);
      default:
        throw ArgumentError.value(data, 'data', 'Invalid data type.');
    }
  }

  @internal
  Future<Object> read(web.Blob blob) async {
    final arrayBuffer = await blob.arrayBuffer().toDart;
    final bytes = arrayBuffer.toDart.asUint8List();
    return bytes;
  }
}

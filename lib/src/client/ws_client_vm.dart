import 'dart:async';
import 'dart:convert';
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/client/web_socket_ready_state.dart';
import 'package:ws/src/client/websocket_exception.dart';
import 'package:ws/src/client/ws_client_base.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/client/ws_options.dart';
import 'package:ws/src/client/ws_options_vm.dart';
import 'package:ws/src/util/logger.dart';

/// Platform related callback to create a WebSocket client.
@internal
IWebSocketClient $platformWebSocketClient(WebSocketOptions? options) =>
    switch (options) {
      $WebSocketOptions$VM options => WebSocketClient$VM(
          interceptors: options.interceptors,
          protocols: options.protocols,
          options: options,
        ),
      _ => WebSocketClient$VM(
          interceptors: options?.interceptors,
          protocols: options?.protocols,
          options: $WebSocketOptions$VM(
            protocols: options?.protocols,
            connectionRetryInterval: options?.connectionRetryInterval,
            timeout: options?.timeout,
            afterConnect: options?.afterConnect,
            interceptors: options?.interceptors,
          ),
        ),
    };

@internal
final class WebSocketClient$VM extends WebSocketClientBase {
  WebSocketClient$VM({
    super.interceptors,
    super.protocols,
    $WebSocketOptions$VM? options,
  }) : _options = options;

  final $WebSocketOptions$VM? _options;

  /// Native WebSocket client.
  // Close it at a [disconnect] or [close] method.
  // ignore: close_sinks
  io.WebSocket? _client;

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
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
  void push(Object data) {
    final client = _client;
    if (client == null || client.readyState != io.WebSocket.open) {
      throw const WSClientClosedException(
          message: 'WebSocket client is not connected.');
    }
    try {
      switch (data) {
        case String text:
          client.addUtf8Text(utf8.encode(text));
        case TypedData bytes:
          client.add(bytes);
        case ByteBuffer bb:
          client.add(bb.asUint8List());
        case List<int> bytes:
          client.add(Uint8List.fromList(bytes));
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
      if (_client != null) await disconnect(1001, 'RECONNECTING');
      super.connect(url);
      if (_options?.userAgent case String userAgent) {
        io.WebSocket.userAgent = userAgent;
      }
      // Close it at a [disconnect] or [close] method.
      // ignore: close_sinks
      final client = _client = await io.WebSocket.connect(
        url,
        protocols: protocols,
        headers: _options?.headers,
        compression:
            _options?.compression ?? io.CompressionOptions.compressionDefault,
        customClient: _options?.customClient,
      );
      // coverage:ignore-start
      _dataBindSubscription = client
          .asyncMap<Object?>((data) => switch (data) {
                String text => text,
                ByteBuffer bb => bb.asUint8List(),
                TypedData td => Uint8List.view(
                    td.buffer,
                    td.offsetInBytes,
                    td.lengthInBytes,
                  ),
                List<int> bytes => bytes,
                _ => data,
              })
          .listen(
            super.onReceivedData,
            onError: onError,
            onDone: () => disconnect(
              _client?.closeCode ?? 1000,
              _client?.closeReason ?? 'SUBSCRIPTION_CLOSED',
            ),
            cancelOnError: false,
          );
      // coverage:ignore-end

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
    _dataBindSubscription?.cancel().ignore();
    _dataBindSubscription = null;
    if (client != null) {
      try {
        await client.close(code, reason);
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

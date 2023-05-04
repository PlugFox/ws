import 'dart:async';
import 'dart:developer';
import 'dart:html' as html
    show WebSocket, Blob, Event, MessageEvent, CloseEvent;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/model/web_socket_ready_state.dart';
import 'package:ws/src/platform/platform.base.dart';
import 'package:ws/src/platform/platform.i.dart';
import 'package:ws/src/util/constants.dart';

/// Get the platform WebSocket transport client for the current environment.
/// {@nodoc}
@internal
IWebSocketPlatformTransport $getWebSocketTransport(String url) =>
    html.WebSocket.supported
        ? WebSocketPlatformTransport$HTML(url)
        : throw UnsupportedError(
            'Cannot create a WebSocket because it is not supported.');

/// WebSocket platform transport for HTML & JS environment.
/// {@nodoc}
final class WebSocketPlatformTransport$HTML = WebSocketPlatformTransport$Base
    with _WebSocketPlatformTransport$HTML$Mixin;

base mixin _WebSocketPlatformTransport$HTML$Mixin
    on WebSocketPlatformTransport$Base {
  /// Native WebSocket client.
  /// {@nodoc}
  html.WebSocket? _communication;

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
  /// {@nodoc}
  StreamSubscription<html.MessageEvent>? _dataBindSubscription;

  /// Binding to error from native WebSocket client.
  /// {@nodoc}
  StreamSubscription<html.Event>? _errorBindSubscription;

  /// Binding to close event from native WebSocket client.
  /// {@nodoc}
  StreamSubscription<html.CloseEvent>? _closeBindSubscription;

  @override
  String? get extensions => _communication?.extensions;

  @override
  WebSocketReadyState get readyState {
    final code = _communication?.readyState;
    assert(code == null || code >= 0 && code <= 3, 'Invalid readyState code.');
    return code == null
        ? WebSocketReadyState.closed
        : WebSocketReadyState.fromCode(code);
  }

  @override
  int? get closeCode => _$closeCode;
  int? _$closeCode;

  @override
  String? get closeReason => _$closeReason;
  String? _$closeReason;

  @override
  Future<void> connect() async {
    try {
      disconnect(1001, 'Reconnecting.');
      _communication = html.WebSocket(url);
      _$closeCode = null;
      _$closeReason = null;
      await _communication?.onOpen.first;
      _errorBindSubscription = _communication?.onError.listen(
        (event) {
          // TODO(plugfox): extract error from event and map it to a WSException
          debugger(when: $kDebugWS);
          receiveError(event);
        },
        onError: receiveError,
        onDone: disconnect,
        cancelOnError: false,
      );
      _dataBindSubscription = _communication?.onMessage.listen(
        (event) {
          final data = event.data;
          if (data is! Object) return;
          receiveData(data);
        },
        onError: receiveError,
        onDone: disconnect,
        cancelOnError: false,
      );
      _closeBindSubscription = _communication?.onClose.listen(
        (event) => disconnect(event.code, event.reason),
        onError: receiveError,
        cancelOnError: false,
      );
      if (!readyState.isOpen) {
        disconnect(1001, 'Is not open after connect.');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      }
    } on Object catch (error, stackTrace) {
      // TODO(plugfox): find out reason for error and map it to a WSException
      debugger(when: $kDebugWS);
      disconnect(1006, 'Connection failed.');
      receiveError(error, stackTrace);
      rethrow;
    }
  }

  @override
  void add(Object data) {
    if (_communication == null) throw StateError('Not connected.');
    try {
      switch (data) {
        case [String text]:
          _communication?.sendString(text);
          break;
        case [TypedData td]:
          _communication?.sendTypedData(td);
          break;
        case [html.Blob blob]:
          _communication?.sendBlob(blob);
          break;
        case [ByteBuffer bb]:
          _communication?.sendByteBuffer(bb);
          break;
        case [List<int> bytes]:
          _communication?.send(bytes);
          break;
        default:
          assert(false, 'Invalid data type: ${data.runtimeType}');
          break;
      }
    } on Object catch (error, stackTrace) {
      // TODO(plugfox): find out reason for error and map it to a WSException
      debugger(when: $kDebugWS);
      receiveError(error, stackTrace);
      rethrow;
    }
  }

  @override
  void disconnect([int? code, String? reason]) {
    _$closeCode = code;
    _$closeReason = reason;
    _errorBindSubscription?.cancel().ignore();
    _closeBindSubscription?.cancel().ignore();
    _dataBindSubscription?.cancel().ignore();
    Future<void>.sync(() => _communication?.close(code, reason)).ignore();
    _communication = null;
  }

  @override
  void close([int? code = 1000, String? reason = 'Normal Closure']) {
    disconnect(code, reason);
    super.close(code, reason);
  }
}

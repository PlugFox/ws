import 'dart:async';
import 'dart:developer';
import 'dart:io' as io show WebSocket;

import 'package:meta/meta.dart';
import 'package:ws/src/model/web_socket_ready_state.dart';
import 'package:ws/src/platform/platform.base.dart';
import 'package:ws/src/platform/platform.i.dart';
import 'package:ws/src/util/constants.dart';

/// Get the platform WebSocket transport client for the current environment.
/// {@nodoc}
@internal
IWebSocketPlatformTransport $getWebSocketTransport(String url) =>
    WebSocketPlatformTransport$IO(url);

/// WebSocket platform transport for I/O environment.
/// {@nodoc}
final class WebSocketPlatformTransport$IO = WebSocketPlatformTransport$Base
    with _WebSocketPlatformTransport$IO$Mixin;

base mixin _WebSocketPlatformTransport$IO$Mixin
    on WebSocketPlatformTransport$Base {
  /// Native WebSocket client.
  /// {@nodoc}
  io.WebSocket? _communication;

  /// Binding to data from native WebSocket client.
  /// The subscription of [_communication] to [_controller].
  /// {@nodoc}
  StreamSubscription<Object?>? _dataBindSubscription;

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
      _communication = await io.WebSocket.connect(url);
      _$closeCode = null;
      _$closeReason = null;
      _dataBindSubscription = _communication?.listen(
        (data) {
          if (data is! Object) return;
          receiveData(data);
        },
        onError: receiveError,
        onDone: disconnect,
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
          _communication?.addUtf8Text(text.codeUnits);
          break;
        case [List<int> bytes]:
          _communication?.add(bytes);
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

import 'dart:async';
import 'dart:developer';
import 'dart:io' as io show WebSocket, SocketException;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/model/web_socket_ready_state.dart';
import 'package:ws/src/model/websocket_exception.dart';
import 'package:ws/src/platform/platform.base.dart';
import 'package:ws/src/platform/platform.i.dart';
import 'package:ws/src/util/constants.dart';

// TODO(plugfox): Добавить состояние и стрим состояния.
// количество отправленных сообщений, общий размер сообщений
// количество полученных сообщений, общий размер сообщений
// скорость отправки
// скорость получения
// последние ошибки и их время
// количество переподключений
// время последнего переподключения
// время последнего отправленного/полученного сообщения
// состояние подключения, время подключения

/// Get the platform WebSocket transport client for the current environment.
/// {@nodoc}
@internal
IWebSocketPlatformTransport $getWebSocketTransport() =>
    WebSocketPlatformTransport$IO();

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
  Future<void> connect(String url) async {
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
        onDone: () => disconnect(1000, 'Subscription closed.'),
        cancelOnError: false,
      );
      if (!readyState.isOpen) {
        disconnect(1001, 'Is not open after connect.');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      }
    } on io.SocketException catch (error, stackTrace) {
      // That error is only for I/O environment.
      final exception = WSSocketException(error.message);
      debugger(when: $kDebugWS);
      disconnect(1006, error.message);
      receiveError(exception, stackTrace);
      Error.throwWithStackTrace(exception, stackTrace);
    } on Object catch (error, stackTrace) {
      // TODO(plugfox): find out reason for error and map it to a WSException
      debugger(when: $kDebugWS);
      disconnect(1006, 'Connection failed.');
      receiveError(error, stackTrace);
      rethrow;
    }
  }

  @override
  FutureOr<void> add(Object data) {
    if (!readyState.isOpen) throw WSNotConnected('Not connected.');

    try {
      switch (data) {
        case String text:
          _communication?.addUtf8Text(text.codeUnits);
          break;
        case TypedData td:
          _communication?.add(td.buffer.asInt8List());
          break;
        case ByteBuffer bb:
          _communication?.add(bb.asInt8List());
          break;
        case List<int> bytes:
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
    assert(
      readyState == WebSocketReadyState.closed,
      'Invalid readyState code after disconnect: $readyState',
    );
  }

  @override
  void close([int? code = 1000, String? reason = 'Normal Closure']) {
    disconnect(code, reason);
    super.close(code, reason);
  }
}

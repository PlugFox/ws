import 'dart:async';
import 'dart:html' as html show WebSocket, Blob;
import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/platform/platform.base.dart';
import 'package:ws/src/platform/platform.i.dart';

@internal
IWebSocketPlatformTransport $getWebSocketTransport(String url) =>
    html.WebSocket.supported
        ? WebSocketPlatformTransport$HTML(url)
        : throw UnsupportedError(
            'Cannot create a WebSocket because it is not supported.');

final class WebSocketPlatformTransport$HTML = WebSocketPlatformTransport$Base
    with _WebSocketPlatformTransport$HTML$Mixin;

base mixin _WebSocketPlatformTransport$HTML$Mixin
    on WebSocketPlatformTransport$Base {
  html.WebSocket? _communication;

  final StreamController<Object> _controller =
      StreamController<Object>.broadcast();

  @override
  late final Stream<Object> stream = _controller.stream;

  StreamSubscription<Object?>? _bindSubscription;

  @override
  bool get isClosed => _controller.isClosed;

  @override
  Future<void> get done => _controller.done;

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
      close(1001, 'Reconnecting.');
      _communication = html.WebSocket(url);
      _$closeCode = null;
      _$closeReason = null;
      _bindSubscription = _communication?.onMessage.listen(
        (event) {
          final data = event.data;
          if (data is! Object) return;
          _controller.add(data);
        },
        onError: _controller.addError,
        onDone: disconnect,
        cancelOnError: false,
      );
      if (!readyState.isOpen) {
        close(1001, 'Is not open after connect.');
        assert(
          false,
          'Invalid readyState code after connect: $readyState',
        );
      }
    } on Object {
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
    } on Object {
      rethrow;
    }
  }

  @override
  void disconnect([int? code, String? reason]) {
    _$closeCode = code;
    _$closeReason = reason;
    _bindSubscription?.cancel().ignore();
    Future<void>.sync(() => _communication?.close(code, reason)).ignore();
    _communication = null;
  }

  @override
  void close([int? code = 1000, String? reason = 'Normal Closure']) {
    disconnect(code, reason);
    _controller.close();
  }
}

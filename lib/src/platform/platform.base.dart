import 'dart:async';
import 'dart:developer';

import 'package:meta/meta.dart';
import 'package:ws/src/model/websocket_exception.dart';
import 'package:ws/src/platform/platform.i.dart';
import 'package:ws/src/util/constants.dart';

/// Base class for platform WebSocket transport.
/// {@nodoc}
abstract base class WebSocketPlatformTransport$Base
    implements IWebSocketPlatformTransport {
  /// {@nodoc}
  WebSocketPlatformTransport$Base(this.url)
      : _controller = StreamController<Object>.broadcast();

  @override
  final String url;

  /// Output stream of data from native WebSocket client.
  /// {@nodoc}
  final StreamController<Object> _controller;

  @override
  late final Stream<Object> stream = _controller.stream;

  @override
  bool get isClosed => _controller.isClosed;

  @override
  Future<void> get done => _controller.done;

  /// Receive data from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  @protected
  void receiveData(Object data) {
    if (isClosed) {
      assert(false, 'Cannot receive data to a closed stream controller.');
    } else {
      _controller.add(data);
    }
  }

  /// Receive error from native WebSocket client.
  /// {@nodoc}
  @nonVirtual
  @protected
  void receiveError(Object error, [StackTrace? stackTrace]) {
    debugger(when: $kDebugWS);
    if (isClosed) {
      assert(false, 'Cannot receive error to a closed stream controller.');
    } else {
      // TODO(plugfox): map all errors to WSException
      switch (error) {
        case WSException e:
          error = e;
          break;
        case String e:
          error = WSException(e);
          break;
        case Exception e:
          debugger(when: $kDebugWS);
          error = WSException(e.toString());
          break;
        case Error e:
          debugger(when: $kDebugWS);
          error = WSException(e.toString());
          break;
        case Object:
          debugger(when: $kDebugWS);
          error = WSException(error.toString());
          break;
      }
      _controller.addError(error, stackTrace);
    }
  }

  @override
  @mustCallSuper
  void close([int? code = 1000, String? reason = 'Normal Closure']) =>
      _controller.close();
}

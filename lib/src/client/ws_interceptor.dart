import 'dart:async';

/// Interceptor for WebSocket messages.
/// All methods should call `next` to continue the chain.
///
/// Hints:
/// 1) You can send a any type of data, and change its type to
/// a string or a bytes array only at last Interceptor in chain.
/// You can send a Map<String, dynamic> and change it to a string
/// or attach a context to the message.
///
/// 2) You can log the message and use performance metrics inside
/// Interceptor.
///
/// 3) You can call "next" once or multiple times or not call it at all
/// to stop the chain.
abstract interface class WSInterceptor {
  /// Create a [WSInterceptor] from a functions.
  factory WSInterceptor.wrap({
    FutureOr<void> Function(Object data)? onSend,
    FutureOr<void> Function(Object data)? onMessage,
  }) = _WSInterceptorWrapper;

  /// Create a [WSInterceptor] from a functions.
  factory WSInterceptor.handlers({
    FutureOr<void> Function(
      Object data,
      void Function(Object data) next,
    )? onSend,
    FutureOr<void> Function(
      Object data,
      void Function(Object data) next,
    )? onMessage,
  }) = _WSInterceptorHandlers;

  /// Called when the message is about to be sent.
  void onSend(Object data, void Function(Object data) next);

  /// Called when the message is about to be received.
  void onMessage(Object data, void Function(Object data) next);
}

class _WSInterceptorWrapper implements WSInterceptor {
  _WSInterceptorWrapper({
    FutureOr<void> Function(Object data)? onSend,
    FutureOr<void> Function(Object data)? onMessage,
  })  : _onSend = onSend,
        _onMessage = onMessage;

  final FutureOr<void> Function(Object data)? _onSend;
  final FutureOr<void> Function(Object data)? _onMessage;

  @override
  Future<void> onSend(Object data, void Function(Object data) next) async {
    _onSend?.call(data);
    next(data);
  }

  @override
  Future<void> onMessage(Object data, void Function(Object data) next) async {
    await _onMessage?.call(data);
    next(data);
  }
}

class _WSInterceptorHandlers implements WSInterceptor {
  _WSInterceptorHandlers({
    FutureOr<void> Function(
      Object data,
      void Function(Object data) next,
    )? onSend,
    FutureOr<void> Function(
      Object data,
      void Function(Object data) next,
    )? onMessage,
  })  : _onSend = onSend,
        _onMessage = onMessage;

  final FutureOr<void> Function(
    Object data,
    void Function(Object data) next,
  )? _onSend;
  final FutureOr<void> Function(
    Object data,
    void Function(Object data) next,
  )? _onMessage;

  @override
  void onSend(Object data, void Function(Object data) next) =>
      _onSend?.call(data, next);

  @override
  void onMessage(Object data, void Function(Object data) next) =>
      _onMessage?.call(data, next);
}

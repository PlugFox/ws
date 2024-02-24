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
  /// Called when the message is about to be sent.
  void onSend(Object data, void Function(Object data) next);

  /// Called when the message is about to be received.
  void onMessage(Object data, void Function(Object data) next);
}

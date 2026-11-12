import 'package:ws/src/client/exceptions.dart';

/// Correlates requests and replies for `WebSocketClient.rpc`.
///
/// The client has no RPC protocol of its own; this handler maps it onto the
/// application's message format.
abstract interface class WebSocketRpc<T> {
  /// Turns [data] into a request message with a correlation id.
  ///
  /// [seq] grows for the life of the client and is never reused, also not
  /// across reconnects; use it as the id or ignore it.
  (Object id, T message) encode(int seq, T data);

  /// Called for every incoming message. Returns the id of the request it
  /// answers, or `null` if it is not a reply (it is then delivered as a
  /// normal message).
  Object? match(T message);

  /// Called for replies only. Returns the result of the call, or throws
  /// (for example a [WebSocketRpcException] with
  /// [WebSocketRpcFailure.remote]) to fail it.
  T decode(T reply);
}

/// RPC over JSON objects: `{"id": 1, ...request}` out, `{"id": 1, ...}`
/// back. A reply with a non-null [errorKey] fails the call.
final class WebSocketJsonRpc implements WebSocketRpc<Map<String, Object?>> {
  /// Creates the handler.
  const WebSocketJsonRpc({this.idKey = 'id', this.errorKey = 'error'});

  /// The key holding the correlation id.
  final String idKey;

  /// The key holding an error; `null` disables error detection.
  final String? errorKey;

  @override
  (Object, Map<String, Object?>) encode(int seq, Map<String, Object?> data) =>
      (seq, <String, Object?>{...data, idKey: seq});

  @override
  Object? match(Map<String, Object?> message) => switch (message[idKey]) {
    // JSON numbers may decode as double; ids are ints we generated.
    final num id => id.toInt(),
    _ => null,
  };

  @override
  Map<String, Object?> decode(Map<String, Object?> reply) {
    final key = errorKey;
    if (key != null && reply[key] != null) {
      throw WebSocketRpcException(
        'Server replied with an error',
        failure: WebSocketRpcFailure.remote,
        id: reply[idKey],
        error: reply[key],
      );
    }
    return reply;
  }
}

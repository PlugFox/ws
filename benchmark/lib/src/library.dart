/// Variants `ws-dart`, `ws-native` and `ws` (browser): the library itself
/// through its public API. Platform-neutral: the caller passes the native
/// transport, if any.
library;

import 'dart:async';
import 'dart:typed_data';

import 'package:ws/transport.dart' show WebSocketTransport;
import 'package:ws/ws.dart';

import 'bench_client.dart';

final class LibraryClient implements BenchClient {
  LibraryClient._(this._send, this._close, this._state);

  final Future<void> Function(Object) _send;
  final Future<void> Function() _close;
  final WebSocketState Function() _state;
  final Completer<void> _done = Completer<void>();
  int? _closeCode;

  static Future<BenchClient> connect(
    Uri uri,
    void Function(Object message) onMessage,
    String format, {
    WebSocketTransport? transport,
  }) async {
    const tls = WebSocketVmOptions(tls: WebSocketTls(allowInsecure: true));
    const reconnect = WebSocketReconnect.disabled();
    late final LibraryClient result;
    void onState(WebSocketState s) {
      if (s is WebSocketState$Disconnected && !result._done.isCompleted) {
        result._closeCode = s.closeCode ?? 1006;
        result._done.complete();
      }
    }

    if (format == 'binary') {
      final client = WebSocketClient.binary(
        options: WebSocketOptions(
          vm: tls,
          reconnect: reconnect,
          transport: transport,
          maxMessageSize: 256 << 20,
          onMessage: onMessage,
          onStateChange: (s) => onState(s),
        ),
      );
      result = LibraryClient._(
        (m) => client.push(m as Uint8List),
        client.close,
        () => client.state,
      );
      await client.connect(uri.toString());
    } else {
      final client = WebSocketClient.json(
        options: WebSocketOptions(
          vm: tls,
          reconnect: reconnect,
          transport: transport,
          maxMessageSize: 256 << 20,
          onMessage: onMessage,
          onStateChange: (s) => onState(s),
        ),
      );
      result = LibraryClient._(
        (m) => client.push(m as Map<String, Object?>),
        client.close,
        () => client.state,
      );
      await client.connect(uri.toString());
    }
    return result;
  }

  @override
  void send(Object message) => _send(message).ignore();

  @override
  Future<void> get done => _done.future;

  @override
  int? get closeCode => _closeCode;

  @override
  Future<void> close() async {
    if (_state().isOpen) await _close();
    if (!_done.isCompleted) _done.complete();
  }
}

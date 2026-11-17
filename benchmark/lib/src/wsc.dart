/// Variant `wsc`: `package:web_socket_channel`, the most used WebSocket
/// package on pub.dev, used the way its README shows: listen to `stream`,
/// `jsonDecode` text frames, `jsonEncode` before `sink.add`.
///
/// Platform-neutral: the caller opens the channel (`IOWebSocketChannel` on
/// the VM, to accept the benchmark server's self-signed certificate;
/// `WebSocketChannel.connect` in browsers).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:web_socket_channel/web_socket_channel.dart';

import 'bench_client.dart';

final class WscClient implements BenchClient {
  WscClient._(this._channel, void Function(Object) onMessage) {
    _channel.stream.listen(
      (Object? data) => switch (data) {
        String text => onMessage(jsonDecode(text) as Object),
        Uint8List bytes => onMessage(bytes),
        ByteBuffer buffer => onMessage(buffer.asUint8List()),
        List<int> bytes => onMessage(Uint8List.fromList(bytes)),
        _ => throw StateError('Unexpected ${data.runtimeType}'),
      },
      onDone: () {
        if (!_done.isCompleted) _done.complete();
      },
      onError: (Object _) {},
    );
  }

  static Future<BenchClient> connect(
    WebSocketChannel channel,
    void Function(Object message) onMessage,
  ) async {
    final client = WscClient._(channel, onMessage);
    await channel.ready;
    return client;
  }

  final WebSocketChannel _channel;
  final Completer<void> _done = Completer<void>();

  @override
  void send(Object message) => switch (message) {
        Uint8List bytes => _channel.sink.add(bytes),
        _ => _channel.sink.add(jsonEncode(message)),
      };

  @override
  Future<void> get done => _done.future;

  @override
  int? get closeCode => _channel.closeCode;

  @override
  Future<void> close() async {
    if (_done.isCompleted) return;
    await _channel.sink.close(1000);
    await done.timeout(const Duration(seconds: 5), onTimeout: () {});
  }
}

// The Rust test server seen from a browser test (server_hybrid.dart).
import 'dart:async';
import 'dart:convert';

import 'package:stream_channel/stream_channel.dart';
import 'package:test/test.dart';

import 'server_handle.dart';

/// The server, or why the tests that need it are skipped.
typedef BrowserServerStart = ({BrowserServer? server, String? skip});

/// [ServerHandle] over a hybrid isolate's channel.
final class BrowserServer implements ServerHandle {
  BrowserServer._(this._channel, this._port, Stream<Object?> replies) {
    replies.listen((message) {
      final reply = jsonDecode(message! as String) as Map<String, Object?>;
      final waiter = _waiters.remove(reply['n']! as int)!;
      switch (reply) {
        case {'log': final Map<String, Object?> log}:
          waiter.complete(log);
        default:
          waiter.completeError(StateError('${reply['error']}'));
      }
    });
  }

  /// Starts the server; call it from `setUpAll`. It stops after the
  /// matching `tearDownAll` (the channel closes).
  static Future<BrowserServerStart> start() async {
    final channel = spawnHybridUri('/test/support/server_hybrid.dart');
    final messages = StreamIterator(channel.stream);
    if (!await messages.moveNext()) {
      throw StateError('the test server hybrid exited');
    }
    final ready =
        jsonDecode(messages.current! as String) as Map<String, Object?>;
    if (ready['error'] case final String error) throw StateError(error);
    if (ready['skip'] case final String skip) return (server: null, skip: skip);
    final rest = StreamController<Object?>();
    unawaited(() async {
      while (await messages.moveNext()) {
        rest.add(messages.current);
      }
      await rest.close();
    }());
    return (
      server: BrowserServer._(channel, ready['ws']! as int, rest.stream),
      skip: null,
    );
  }

  final StreamChannel<Object?> _channel;
  final int _port;
  final Map<int, Completer<Map<String, Object?>>> _waiters = {};
  var _next = 0;

  @override
  String url(String path) => 'ws://127.0.0.1:$_port$path';

  @override
  Future<Map<String, Object?>> log(String id, {bool done = true}) {
    final n = _next++;
    final waiter = _waiters[n] = Completer<Map<String, Object?>>();
    _channel.sink.add({'n': n, 'log': id, 'done': done});
    return waiter.future;
  }
}

// Runs the Rust test server for browser tests (spawnHybridUri runs this on
// the VM). Protocol on the channel:
//   first message: {"ws": port}, {"skip": reason} or {"error": message}
//   request {"n": k, "log": id, "done": bool} -> reply {"n": k, "log": {...}}
// Replies are JSON strings: maps crossing the channel lose their types
// in the browser.
// The server stops when the channel closes.
import 'dart:async';
import 'dart:convert';

import 'package:stream_channel/stream_channel.dart';

import 'test_server.dart';

Future<void> hybridMain(StreamChannel<Object?> channel) async {
  final Object skip;
  try {
    skip = TestServer.skip;
  } on Object catch (e) {
    // On CI a missing server fails the suite instead of skipping it.
    channel.sink.add(jsonEncode({'error': '$e'}));
    return;
  }
  if (skip is String) {
    channel.sink.add(jsonEncode({'skip': skip}));
    return;
  }
  final server = await TestServer.start();
  channel.sink.add(jsonEncode({'ws': server.ws}));
  await for (final message in channel.stream) {
    final request = message! as Map<String, Object?>;
    final id = request['log']! as String;
    unawaited(
      server
          .log(id, done: request['done'] != false)
          .then(
            (log) =>
                channel.sink.add(jsonEncode({'n': request['n'], 'log': log})),
            onError: (Object e) => channel.sink.add(
              jsonEncode({'n': request['n'], 'error': '$e'}),
            ),
          ),
    );
  }
  await server.stop();
}

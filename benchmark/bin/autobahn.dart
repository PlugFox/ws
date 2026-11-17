/// Autobahn TestSuite client (fuzzingserver) for the ws transports.
///
///   docker run -d --rm -p 9001:9001 \
///     -v $PWD/autobahn:/config -v $PWD/build/autobahn:/reports \
///     --name fuzzingserver crossbario/autobahn-testsuite \
///     wstest -m fuzzingserver -s /config/fuzzingserver.json
///   dart run bin/autobahn.dart --transport=dart|native [--cases=1.1.1,...]
///
/// Runs at the transport level (what Autobahn tests is the protocol): every
/// message is echoed with the same type. Text is validated like the
/// client's codecs do (invalid UTF-8 → close 1007). Cases 12.* and 13.*
/// (permessage-deflate) are excluded: not supported yet (ROADMAP §4.4).
library;

import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:ws/src/protocol/utf8_validator.dart';
import 'package:ws/src/transport/dart/dart_transport.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';
import 'package:ws_native/ws_native.dart';

const _base = 'ws://127.0.0.1:9001';
bool _verbose = false;

Future<void> main(List<String> arguments) async {
  final args = {
    for (final a in arguments)
      if (a.startsWith('--'))
        a.substring(2).split('=').first:
            a.contains('=') ? a.substring(a.indexOf('=') + 1) : 'true',
  };
  final name = args['transport'] ?? 'dart';
  final WebSocketTransport transport;
  if (name == 'native') {
    if (!WsNative.install()) throw StateError('ws_native not available');
    transport = WsNative.library!.transport();
  } else {
    transport = const DartTransport();
  }
  final agent = 'ws-$name';
  _verbose = args['verbose'] == 'true';

  final count = int.parse(await _fetchText(transport, '$_base/getCaseCount'));
  final only = args['cases']?.split(',').toSet();
  stderr.writeln('$count cases, agent $agent');
  for (var i = 1; i <= count; i++) {
    final info = await _fetchText(transport, '$_base/getCaseInfo?case=$i');
    final id = (jsonDecode(info) as Map<String, Object?>)['id'] as String;
    if (only != null && !only.contains(id)) continue;
    stderr.write('\r\x1B[K$i/$count $id');
    await _runCase(transport, '$_base/runCase?case=$i&agent=$agent')
        .timeout(const Duration(seconds: 60), onTimeout: () {
      stderr.writeln('\ncase $id timed out');
    });
  }
  stderr.writeln();
  await _runCase(transport, '$_base/updateReports?agent=$agent');
  stderr.writeln('Reports: build/autobahn/index.html');
}

/// Echoing into a connection the server already failed is expected.
void _try(void Function() send) {
  try {
    send();
  } on WebSocketNotConnectedException {
    // closed
  }
}

final class _Echo implements TransportListener {
  _Echo(this.done, {this.onText});

  late TransportConnection connection;
  final Completer<void> done;
  final void Function(String text)? onText;

  @override
  void onOpen(String? protocol) {
    if (_verbose) stderr.writeln('\n  open');
  }

  @override
  void onTextBytes(Uint8List bytes) {
    if (!Utf8Validator.validate(bytes)) {
      connection.close(1007, 'Invalid UTF-8');
      return;
    }
    if (onText != null) return onText!(utf8.decode(bytes));
    _try(() => connection.sendTextBytes(Uint8List.fromList(bytes)));
  }

  @override
  void onTextString(String text) =>
      onText != null ? onText!(text) : _try(() => connection.sendText(text));

  @override
  void onJson(Object? value) => throw StateError('raw mode only');

  @override
  void onBinary(Uint8List bytes) {
    if (_verbose) stderr.writeln('\n  binary ${bytes.length}');
    _try(() => connection.sendBinary(Uint8List.fromList(bytes)));
    if (_verbose) stderr.writeln('  buffered ${connection.bufferedAmount}');
  }

  @override
  void onPong() {}

  @override
  void onDrain() {}

  @override
  void onClosed(int? code, String reason, WebSocketException? error) {
    if (error != null && _verbose) stderr.writeln('\n  closed: $code $reason $error');
    if (!done.isCompleted) done.complete();
  }
}

Future<void> _runCase(WebSocketTransport transport, String url) {
  final done = Completer<void>();
  final echo = _Echo(done);
  echo.connection = transport.connect(
    TransportRequest(url: Uri.parse(url), maxMessageSize: 64 << 20),
    echo,
  );
  return done.future;
}

Future<String> _fetchText(WebSocketTransport transport, String url) async {
  final done = Completer<void>();
  String? text;
  final echo = _Echo(done, onText: (t) => text = t);
  echo.connection = transport.connect(
    TransportRequest(url: Uri.parse(url)),
    echo,
  );
  await done.future;
  return text ?? (throw StateError('No reply from $url'));
}

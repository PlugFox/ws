// The contract checker must catch what it claims to catch: a transport
// that breaks each rule once.
import 'dart:async';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

import '../support/checked_transport.dart';

void main() {
  Future<List<String>> violations(
    void Function(TransportListener l, _Broken c) script, {
    void Function(TransportConnection c)? use,
  }) async {
    final checked = CheckedTransport(_BrokenTransport(script));
    final c = checked.connect(
      TransportRequest(url: Uri.parse('ws://x')),
      _Null(),
    );
    use?.call(c);
    await Future<void>.delayed(Duration.zero);
    final found = [...checked.violations];
    await expectLater(
      checked.verify(timeout: const Duration(milliseconds: 50)),
      found.isEmpty && checked.violations.isEmpty
          ? completes
          : throwsA(isA<TestFailure>()),
    );
    return found;
  }

  test('a well-behaved connection passes', () async {
    expect(
      await violations((l, c) {
        c.later(() {
          l
            ..onOpen(null)
            ..onBinary(Uint8List(1))
            ..onClosed(1000, '', null);
        });
      }),
      isEmpty,
    );
  });

  test('a synchronous callback from connect', () async {
    expect(await violations((l, c) => l.onClosed(null, '', null)), [
      startsWith('synchronous from connect: onClosed'),
    ]);
  });

  test('a synchronous callback from a connection method', () async {
    expect(
      await violations((l, c) {
        c
          ..later(() => l.onOpen(null))
          ..onAbort = () => l.onClosed(null, '', null);
      }, use: (c) => Timer.run(c.abort)),
      [startsWith('synchronous from abort: onClosed')],
    );
  });

  test('events after onClosed, onOpen twice, messages before open', () async {
    expect(
      await violations((l, c) {
        c.later(() {
          l
            ..onTextBytes(Uint8List(0))
            ..onOpen(null)
            ..onOpen(null)
            ..onClosed(null, '', null)
            ..onPong();
        });
      }),
      [
        startsWith('before onOpen: onTextBytes'),
        startsWith('onOpen twice'),
        startsWith('after onClosed: onPong'),
      ],
    );
  });

  test('onDrain without requestDrain', () async {
    expect(
      await violations((l, c) {
        c.later(() {
          l
            ..onOpen(null)
            ..onDrain()
            ..onClosed(null, '', null);
        });
      }),
      [startsWith('onDrain without requestDrain')],
    );
  });

  test('a connection that never closes', () async {
    final checked = CheckedTransport(
      _BrokenTransport((l, c) => c.later(() => l.onOpen(null))),
    )..connect(TransportRequest(url: Uri.parse('ws://x')), _Null());
    await expectLater(
      checked.verify(timeout: const Duration(milliseconds: 50)),
      throwsA(
        isA<TestFailure>().having(
          (e) => e.message,
          'message',
          contains('no onClosed'),
        ),
      ),
    );
  });
}

final class _BrokenTransport implements WebSocketTransport {
  _BrokenTransport(this.script);

  final void Function(TransportListener l, _Broken c) script;

  @override
  String get name => 'broken';

  @override
  bool get supportsPing => true;

  @override
  bool get decodesJson => false;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) {
    final c = _Broken();
    script(listener, c);
    return c;
  }
}

final class _Broken implements TransportConnection {
  void Function()? onAbort;

  void later(void Function() f) => scheduleMicrotask(f);

  @override
  void abort() => onAbort?.call();

  @override
  int get bufferedAmount => 0;

  @override
  void close(int? code, String? reason) {}

  @override
  void ping() {}

  @override
  void requestDrain() {}

  @override
  void sendBinary(Uint8List bytes) {}

  @override
  void sendText(String text) {}

  @override
  void sendTextBytes(Uint8List bytes) {}
}

final class _Null implements TransportListener {
  @override
  void onBinary(Uint8List bytes) {}
  @override
  void onClosed(int? code, String reason, WebSocketException? error) {}
  @override
  void onDrain() {}
  @override
  void onJson(Object? value) {}
  @override
  void onOpen(String? protocol) {}
  @override
  void onPong() {}
  @override
  void onTextBytes(Uint8List bytes) {}
  @override
  void onTextString(String text) {}
}

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ws/testing.dart';
import 'package:ws/ws.dart';
import 'package:ws_flutter/ws_flutter.dart';

/// Records the lifecycle calls.
final class _Client extends Fake implements WebSocketClient<String> {
  final List<String> calls = <String>[];

  @override
  void suspend({bool disconnect = false}) =>
      calls.add(disconnect ? 'suspend(disconnect)' : 'suspend');

  @override
  Future<void> resume() {
    calls.add('resume');
    return Future<void>.error(
      const WebSocketNotConnectedException('ignored by the observer'),
    );
  }
}

/// Reports [states] one by one, as the engine does.
void _go(WidgetTester tester, List<AppLifecycleState> states) {
  for (final state in states) {
    tester.binding.handleAppLifecycleStateChanged(state);
  }
}

const _toBackground = [
  AppLifecycleState.inactive,
  AppLifecycleState.hidden,
  AppLifecycleState.paused,
];
const _toForeground = [
  AppLifecycleState.hidden,
  AppLifecycleState.inactive,
  AppLifecycleState.resumed,
];

void main() {
  testWidgets('mobile: suspend in the background, resume on return', (
    tester,
  ) async {
    final client = _Client();
    final lifecycle = WebSocketLifecycle(client);
    _go(tester, _toBackground);
    expect(client.calls, ['suspend']);
    _go(tester, _toForeground);
    expect(client.calls, ['suspend', 'resume']);
    lifecycle.dispose();
  });

  testWidgets('disconnectInBackground closes the connection', (tester) async {
    final client = _Client();
    final lifecycle = WebSocketLifecycle(client, disconnectInBackground: true);
    _go(tester, _toBackground);
    expect(client.calls, ['suspend(disconnect)']);
    lifecycle.dispose();
  });

  testWidgets('inactive only (a dialog, the notification shade): nothing', (
    tester,
  ) async {
    final client = _Client();
    final lifecycle = WebSocketLifecycle(client);
    _go(tester, [AppLifecycleState.inactive, AppLifecycleState.resumed]);
    expect(client.calls, isEmpty);
    lifecycle.dispose();
  });

  testWidgets('desktop and web: hidden is checked on return, not suspended', (
    tester,
  ) async {
    final client = _Client();
    final lifecycle = WebSocketLifecycle(client);
    _go(tester, [
      AppLifecycleState.inactive,
      AppLifecycleState.hidden,
      AppLifecycleState.inactive,
      AppLifecycleState.resumed,
    ]);
    expect(client.calls, ['resume']);
    lifecycle.dispose();
  });

  testWidgets('network changes are checked in the foreground only', (
    tester,
  ) async {
    final client = _Client();
    final network = StreamController<Object?>.broadcast();
    final lifecycle = WebSocketLifecycle(
      client,
      networkChanges: network.stream,
    );
    network.add('wifi');
    await tester.pump();
    expect(client.calls, ['resume']);

    _go(tester, _toBackground);
    network
      ..add('mobile')
      ..addError(StateError('the source failed'));
    await tester.pump();
    expect(client.calls, ['resume', 'suspend']);

    _go(tester, _toForeground);
    network.add('none');
    await tester.pump();
    expect(client.calls, ['resume', 'suspend', 'resume', 'resume']);

    lifecycle.dispose();
    expect(network.hasListener, isFalse);
    _go(tester, _toBackground);
    expect(client.calls, hasLength(4));
    lifecycle.dispose(); // twice is fine
    unawaited(network.close());
  });

  testWidgets('with a real client: no pings in the background, one on return', (
    tester,
  ) async {
    final server = FakeWebSocketServer();
    final client = WebSocketClient.text(
      options: WebSocketOptions(
        transport: server,
        heartbeat: const WebSocketHeartbeat(
          interval: Duration(seconds: 10),
          timeout: Duration(seconds: 5),
        ),
      ),
    );
    final lifecycle = WebSocketLifecycle(client);
    unawaited(client.connect('ws://t'));
    await tester.pump();
    final conn = server.last..autoPong = false;

    _go(tester, _toBackground);
    await tester.pump(const Duration(minutes: 10));
    expect(conn.pings, 0);
    expect(client.state.isOpen, isTrue);

    // The OS killed the socket meanwhile: no pong, replaced at once.
    _go(tester, _toForeground);
    await tester.pump();
    expect(conn.pings, 1);
    await tester.pump(const Duration(seconds: 5));
    expect(conn.isEnded, isTrue);
    expect(server.connections, hasLength(2));
    expect(client.state.isOpen, isTrue);

    lifecycle.dispose();
    await client.close();
    await tester.pump(const Duration(seconds: 5));
  });
}

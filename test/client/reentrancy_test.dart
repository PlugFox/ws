// Re-entrancy and randomized operation sequences over the fake transport
// (docs/review/tests_docs.md §4 item 14; TEST_PLAN C22, C24). D22 promises
// that the client survives calls from its own callbacks; these tests call
// every operation from `onStateChange` in every state, then run seeded
// random sequences and check the invariants after every step.
import 'dart:async';
import 'dart:math' as math;

import 'package:fake_async/fake_async.dart';
import 'package:test/test.dart';
import 'package:ws/testing.dart';
import 'package:ws/ws.dart';

/// Watches one client: every future it returned, every state it emitted.
final class _Harness {
  _Harness(this.server, {void Function(WebSocketState)? onState}) {
    client = WebSocketClient.text(
      options: WebSocketOptions(
        transport: server,
        reconnect: const WebSocketReconnect(
          minDelay: Duration(milliseconds: 50),
          maxDelay: Duration(seconds: 2),
        ),
        heartbeat: const WebSocketHeartbeat(
          interval: Duration(seconds: 1),
          timeout: Duration(milliseconds: 500),
        ),
        disconnectTimeout: const Duration(milliseconds: 300),
        onStateChange: (s) => onState?.call(s),
        onError: (e, _) => errors.add(e),
      ),
    );
    client.states.listen(states.add, onDone: () => statesDone = true);
  }

  final FakeWebSocketServer server;
  late final WebSocketClient<String> client;
  final List<WebSocketState> states = [];
  final List<Object> errors = [];
  bool statesDone = false;
  final List<(String, bool Function())> _pending = [];

  /// Records [future] (or a synchronous throw) so that the end of the
  /// test can check it completed.
  void track(String what, Future<void> Function() call) {
    var done = false;
    try {
      call().then((_) => done = true, onError: (Object _) => done = true);
    } on Object {
      done = true;
    }
    _pending.add((what, () => done));
  }

  /// Set to the connection count by disconnect() or close(), cleared by
  /// connect(): no connection may start in between.
  int? frozenAt;

  void wantConnection({required bool wanted}) {
    if (wanted) {
      if (!client.state.isClosed) frozenAt = null;
    } else {
      frozenAt ??= server.connections.length;
    }
  }

  /// Fake connections the client has not closed and that have not ended.
  int get live => server.connections
      .where((c) => !c.isEnded && c.clientClose == null)
      .length;

  void checkInvariants(String step) {
    final closedAt = states.indexWhere((s) => s.isClosed);
    expect(
      closedAt == -1 || closedAt == states.length - 1,
      isTrue,
      reason: '$step: a state after closed: $states',
    );
    expect(live, lessThanOrEqualTo(1), reason: '$step: two live connections');
    if (frozenAt case final n?) {
      expect(
        server.connections.length,
        n,
        reason: '$step: a connection started after disconnect()',
      );
    }
    if (client.state.isOpen) {
      expect(
        server.connections.last.isOpen,
        isTrue,
        reason: '$step: open on a dead connection',
      );
    }
  }

  /// Closes the client, lets every timer run, and checks the end state.
  void finish(FakeAsync async, String step) {
    track('close', client.close);
    async
      ..elapse(const Duration(minutes: 15))
      ..flushMicrotasks();
    checkInvariants(step);
    expect(client.state.isClosed, isTrue, reason: step);
    expect(statesDone, isTrue, reason: '$step: states stream not done');
    final stuck = [
      for (final (what, done) in _pending)
        if (!done()) what,
    ];
    expect(stuck, isEmpty, reason: '$step: futures never completed');
    expect(
      server.connections.where((c) => !c.isEnded),
      isEmpty,
      reason: '$step: a connection outlived close()',
    );
  }
}

void main() {
  group('an operation from onStateChange in each state', () {
    // Every state the scenario below passes through.
    const targets = {
      'connecting': WebSocketState$Connecting,
      'open': WebSocketState$Open,
      'reconnecting': WebSocketState$Reconnecting,
      'disconnecting': WebSocketState$Disconnecting,
      'disconnected': WebSocketState$Disconnected,
      'suspended': WebSocketState$Suspended,
    };
    final actions = <String, void Function(_Harness h)>{
      'connect(other url)': (h) =>
          h.track('connect', () => h.client.connect('ws://other')),
      'connect(same url)': (h) =>
          h.track('connect', () => h.client.connect('ws://a')),
      'disconnect()': (h) => h.track('disconnect', h.client.disconnect),
      'reconnect()': (h) => h.track('reconnect', h.client.reconnect),
      'suspend()': (h) => h.client.suspend(),
      'suspend(disconnect)': (h) => h.client.suspend(disconnect: true),
      'resume()': (h) => h.track('resume', h.client.resume),
      'push()': (h) => h.track('push', () => h.client.push('x')),
      'close()': (h) => h.track('close', h.client.close),
    };

    for (final MapEntry(key: state, value: type) in targets.entries) {
      for (final MapEntry(key: name, value: action) in actions.entries) {
        test('$name in $state', () {
          FakeAsync().run((async) {
            late _Harness h;
            var fired = false;
            h = _Harness(
              FakeWebSocketServer(),
              onState: (s) {
                if (!fired && s.runtimeType == type) {
                  fired = true;
                  action(h);
                }
              },
            );
            void step(String what, void Function() body) {
              body();
              async.flushMicrotasks();
              h.checkInvariants('$name in $state, $what');
            }

            // connecting → open → (drop) reconnecting → open → suspended →
            // resumed → disconnecting → disconnected.
            step(
              'connect',
              () => h.track('connect', () => h.client.connect('ws://a')),
            );
            step('drop', () {
              // None when the action stopped the first attempt early.
              if (h.server.connections.lastOrNull?.isOpen ?? false) {
                h.server.connections.last.drop();
              }
            });
            step('wait', () => async.elapse(const Duration(seconds: 3)));
            step('suspend', () => h.client.suspend(disconnect: true));
            step('resume', () => h.track('resume', h.client.resume));
            step('wait', () => async.elapse(const Duration(seconds: 3)));
            step(
              'disconnect',
              () => h.track('disconnect', h.client.disconnect),
            );
            step('wait', () => async.elapse(const Duration(seconds: 1)));
            expect(fired, isTrue, reason: '$state never reached');
            h.finish(async, '$name in $state');
          });
        });
      }
    }
  });

  group('seeded random sequences', () {
    // Fixed seeds, so a failure reproduces. 5000 were run once (D49).
    const seeds = 300;
    for (var seed = 0; seed < seeds; seed++) {
      test('seed $seed', () => _random(seed));
    }
  });
}

void _random(int seed) => FakeAsync().run((async) {
  final r = math.Random(seed);
  const urls = ['ws://a', 'ws://b', 'ws://c'];
  final log = <String>[];
  late _Harness h;

  void op(String name, void Function() body) {
    log.add(name);
    body();
  }

  // The client calls these back from onStateChange too (re-entrancy).
  void clientOp() {
    switch (r.nextInt(7)) {
      case 0:
        final url = urls[r.nextInt(urls.length)];
        op('connect($url)', () {
          h
            ..wantConnection(wanted: true)
            ..track(url, () => h.client.connect(url));
        });
      case 1:
        op('disconnect', () {
          h
            ..wantConnection(wanted: false)
            ..track('disconnect', h.client.disconnect);
        });
      case 2:
        op('reconnect', () => h.track('reconnect', h.client.reconnect));
      case 3:
        final d = r.nextBool();
        op('suspend($d)', () => h.client.suspend(disconnect: d));
      case 4:
        op('resume', () => h.track('resume', h.client.resume));
      case 5:
        op('push', () => h.track('push', () => h.client.push('m')));
      case 6:
        op('ping', () => h.track('ping', h.client.ping));
    }
  }

  h = _Harness(
    FakeWebSocketServer(),
    onState: (s) {
      if (r.nextInt(8) == 0) {
        log.add('  from $s:');
        clientOp();
      }
    },
  );

  for (var i = 0; i < 60; i++) {
    final last = h.server.connections.isEmpty
        ? null
        : h.server.connections.last;
    switch (r.nextInt(14)) {
      case < 5:
        clientOp();
      case 5 when last != null && !last.isEnded && !last.isOpen:
        if (r.nextBool()) {
          op('accept', last.accept);
        } else {
          final status = const [403, 429, 503][r.nextInt(3)];
          op('reject($status)', () => last.reject(status));
        }
      case 6 when last != null && last.isOpen:
        op('drop', last.drop);
      case 7 when last != null && last.isOpen:
        final code = const [1000, 1001, 1008, 1013, 4000][r.nextInt(5)];
        op('server close($code)', () => last.close(code));
      case 8 when last != null && last.isOpen:
        op('server send', () => last.sendText('s$i'));
      case 9:
        final auto = r.nextInt(3) != 0;
        op('autoAccept=$auto', () => h.server.autoAccept = auto);
      case 10 when r.nextInt(20) == 0:
        op('close', () {
          h
            ..wantConnection(wanted: false)
            ..track('close', h.client.close);
        });
      default:
        final ms = r.nextInt(3000);
        op('elapse($ms)', () => async.elapse(Duration(milliseconds: ms)));
    }
    async.flushMicrotasks();
    h.checkInvariants('seed $seed after ${log.join(', ')}');
  }
  h.finish(async, 'seed $seed after ${log.join(', ')}');
});

import 'dart:async';
import 'dart:convert';

import 'package:game_client/src/leaderboard.dart';
import 'package:game_client/src/online.dart';
import 'package:test/test.dart';

void main() {
  group('online poller', () {
    test('reports the count and the round trip', () async {
      final seen = <OnlineStatus>[];
      final reply = Completer<Map<String, Object?>>();
      final poller = OnlinePoller(() => reply.future, seen.add, every: const Duration(hours: 1))..start();
      await Future<void>.delayed(const Duration(milliseconds: 30));
      reply.complete({'online': 7, 'server_time': 0});
      await pumpEventQueue();
      expect(seen, hasLength(1));
      expect(seen.single.online, 7);
      expect(seen.single.pingMs, greaterThanOrEqualTo(25));
      expect(seen.single.toJson(), {'online': 7, 'pingMs': seen.single.pingMs});
      expect(poller.last, same(seen.single));
      poller.stop();
    });

    test('a failure reads as down; an answer after stop is dropped', () async {
      final seen = <OnlineStatus>[];
      var fail = true;
      final late = Completer<Map<String, Object?>>();
      final poller = OnlinePoller(
        () => fail ? Future.error(StateError('offline')) : late.future,
        seen.add,
        every: const Duration(hours: 1),
      )..start();
      await pumpEventQueue();
      expect(seen.single.down, isTrue);
      expect(seen.single.toJson(), {'online': 0, 'down': true});
      fail = false;
      final next = poller.poll();
      poller.stop();
      late.complete({'online': 3});
      await next;
      expect(seen, hasLength(1), reason: 'left the screen: no more updates');
    });

    test('polls on its period and skips a tick while one is in flight', () async {
      var calls = 0;
      final hang = Completer<Map<String, Object?>>();
      final poller = OnlinePoller(() {
        calls++;
        return hang.future;
      }, (_) {}, every: const Duration(milliseconds: 10))..start();
      await Future<void>.delayed(const Duration(milliseconds: 60));
      expect(calls, 1, reason: 'the first request has not answered');
      hang.complete({'online': 1});
      await Future<void>.delayed(const Duration(milliseconds: 35));
      expect(calls, greaterThan(1));
      poller.stop();
      final stopped = calls;
      await Future<void>.delayed(const Duration(milliseconds: 35));
      expect(calls, stopped);
    });
  });

  group('leaderboard', () {
    test('asks for the page with the own row and adds the online count', () async {
      final asked = <String>[];
      final shown = <Map<String, Object?>>[];
      final board = LeaderboardClient(
        (path) async {
          asked.add(path);
          return {'sort': 'kills', 'page': 2, 'pages': 3, 'total': 25, 'entries': const <Object?>[], 'me': null};
        },
        (json) => shown.add(jsonDecode(json) as Map<String, Object?>),
        me: () => 42,
        online: () => 9,
      );
      await board.query({'type': 'leaderboard_query', 'sort': 'kills', 'page': 2, 'limit': 10});
      expect(asked, ['/api/leaderboard?sort=kills&page=2&limit=10&me=42']);
      expect(shown.single['online'], 9);
      expect(shown.single['pages'], 3);
    });

    test('sanitizes the query and drops a stale answer', () async {
      final asked = <String>[];
      final replies = <Completer<Map<String, Object?>>>[];
      final shown = <Map<String, Object?>>[];
      final board = LeaderboardClient(
        (path) {
          asked.add(path);
          replies.add(Completer());
          return replies.last.future;
        },
        (json) => shown.add(jsonDecode(json) as Map<String, Object?>),
        me: () => null,
        online: () => null,
      );
      final first = board.query({'sort': 'nope', 'page': -3, 'limit': 900});
      final second = board.query({'sort': 'zeny', 'page': 1});
      expect(asked, ['/api/leaderboard?sort=level&page=1&limit=50', '/api/leaderboard?sort=zeny&page=1&limit=10']);
      replies[1].complete({'sort': 'zeny', 'page': 1});
      replies[0].complete({'sort': 'level', 'page': 1});
      await Future.wait([first, second]);
      expect(shown.map((s) => s['sort']), ['zeny'], reason: 'the older answer came late');
      expect(shown.single.containsKey('online'), isFalse);
    });

    test('a failed request shows an empty board with a note', () async {
      final shown = <Map<String, Object?>>[];
      final board = LeaderboardClient(
        (_) => Future.error(StateError('500')),
        (json) => shown.add(jsonDecode(json) as Map<String, Object?>),
        me: () => 1,
        online: () => 2,
      );
      await board.query({'sort': 'mvp', 'page': 4});
      expect(shown.single, containsPair('sort', 'mvp'));
      expect(shown.single, containsPair('page', 4));
      expect(shown.single['entries'], isEmpty);
      expect(shown.single['error'], isNotNull);
    });
  });
}

import 'dart:math' as math;

import 'package:game_client/src/click_move.dart';
import 'package:game_client/src/prediction.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/walk_grid.dart';
import 'package:test/test.dart';

/// A wall with one gap at the bottom, a pillar in the east room, and a
/// walled-in island (x 13–14, y 2–3) nobody reaches.
final room = WalkGrid.ascii([
  '#################',
  '#....#.....#....#',
  '#....#..#..#.#..#',
  '#....#.....#.#..#',
  '#..........#....#',
  '#################',
]);

const rules = MoveRules();

double dist(double ax, double ay, double bx, double by) => math.sqrt((ax - bx) * (ax - bx) + (ay - by) * (ay - by));

/// Ticks [w] from (x, y) with the server movement rules (the move goes
/// through the wire quantisation, as the session sends it) until it is
/// done or [ticks] run out; [each] may move the goal between ticks.
({double x, double y, WalkStatus status, int ticks}) run(
  Walker w,
  WalkGrid grid,
  double x,
  double y, {
  int ticks = 400,
  void Function(int tick, double x, double y)? each,
}) {
  var status = WalkStatus.walking;
  var t = 0;
  for (; t < ticks; t++) {
    each?.call(t, x, y);
    status = w.next(x, y, t / 20, stepLen: rules.step);
    if (status != WalkStatus.walking) break;
    final (dx, dy) = quantizeDir(w.dx, w.dy);
    (x, y) = rules.apply(grid, x, y, dx, dy);
  }
  return (x: x, y: y, status: status, ticks: t);
}

void main() {
  group('WalkGrid for click to move', () {
    test('nearestWalkable: the point itself, else the nearest open cell', () {
      expect(room.nearestWalkable(2.3, 2.7), (2.3, 2.7));
      // Inside the wall at x = 5: the open cell beside it.
      final (x, y) = room.nearestWalkable(5.2, 3.5)!;
      expect(room.isWalkable(x, y), isTrue);
      expect(dist(x, y, 5.2, 3.5), lessThanOrEqualTo(1.0));
      expect(WalkGrid.ascii(['###', '###']).nearestWalkable(1.5, 0.5), isNull);
    });

    test('findPath(closest): an unreachable goal leads to the nearest reachable cell', () {
      expect(room.findPath(2.5, 3.5, 13.5, 2.5, 0.5), isNull);
      final path = room.findPath(2.5, 3.5, 13.5, 2.5, 0.5, closest: true)!;
      expect(path, isNotEmpty);
      final (ex, ey) = path.last;
      expect(room.isWalkable(ex, ey), isTrue);
      // The east room's last column before the wall (x = 10).
      expect(ex, 10.5);
      // Already as near as it gets: nowhere to go.
      expect(room.findPath(10.5, 2.5, 13.5, 2.5, 0.5, closest: true), isEmpty);
    });
  });

  group('Walker', () {
    test('open ground: straight there, stopping on the clicked point', () {
      final w = Walker();
      final end = w.start(room, 1.5, 1.5, WalkGoal.ground, 3.8, 3.2, now: 0);
      expect(end, (3.8, 3.2));
      final r = run(w, room, 1.5, 1.5);
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, 3.8, 3.2), lessThan(0.03), reason: 'the last step is shortened');
      expect(w.active, isFalse);
    });

    test('around the wall to a point behind it, every leg clear', () {
      final w = Walker();
      w.start(room, 2.5, 3.5, WalkGoal.ground, 9.3, 3.4, now: 0);
      var lowest = 9.0;
      final r = run(w, room, 2.5, 3.5, each: (_, x, y) => lowest = math.min(lowest, y));
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, 9.3, 3.4), lessThan(0.05));
      expect(lowest, lessThan(2), reason: 'through the gap in row y = 1');
    });

    test('a click on a wall walks to the open cell nearest to it', () {
      final w = Walker();
      final end = w.start(room, 2.5, 3.5, WalkGoal.ground, 5.4, 3.5, now: 0)!;
      expect(room.isWalkable(end.$1, end.$2), isTrue);
      final r = run(w, room, 2.5, 3.5);
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, end.$1, end.$2), lessThan(0.05));
    });

    test('an unreachable click stops at the edge of the walkable area', () {
      final w = Walker();
      final end = w.start(room, 2.5, 3.5, WalkGoal.ground, 13.5, 2.5, now: 0)!;
      expect(end.$1, 10.5, reason: 'stops short: the marker shows it');
      final r = run(w, room, 2.5, 3.5);
      expect(r.status, WalkStatus.arrived);
      expect(r.x, closeTo(10.5, 0.05));
    });

    test('an NPC on a blocked cell: arrives within its reach', () {
      final w = Walker();
      expect(w.start(room, 2.5, 3.5, WalkGoal.npc, 8.5, 3.5, reach: 1.5, target: 'kafra', now: 0), isNotNull);
      expect(w.target, 'kafra');
      final r = run(w, room, 2.5, 3.5);
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, 8.5, 3.5), lessThanOrEqualTo(1.5));
    });

    test('chasing a moving monster: replans and stops in weapon reach', () {
      final w = Walker();
      var (mx, my) = (9.5, 1.5);
      w.start(room, 1.5, 1.5, WalkGoal.monster, mx, my, reach: 2.45, target: (3, 1), now: 0);
      final r = run(
        w,
        room,
        1.5,
        1.5,
        each: (t, x, y) {
          // It wanders north-east along the east room.
          if (t < 40) mx = math.min(10.5, mx + 0.03);
          my = math.min(4.5, my + 0.04);
          w.retarget(room, x, y, mx, my, now: t / 20, slack: 1);
        },
      );
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, w.tx, w.ty), greaterThan(0), reason: 'stopped');
      expect(dist(r.x, r.y, mx, my), lessThanOrEqualTo(2.45 + 1.0 + 0.3));
    });

    test('steering: follows the pointer, waits at the end while held, walks on', () {
      final w = Walker();
      w.start(room, 1.5, 1.5, WalkGoal.ground, 3.5, 1.5, now: 0);
      w.steering = true;
      var r = run(w, room, 1.5, 1.5, ticks: 60);
      expect(r.status, WalkStatus.walking, reason: 'held: never done');
      expect(w.active, isTrue);
      expect(dist(r.x, r.y, 3.5, 1.5), lessThan(0.05));
      // The pointer moves within the slack: no replanning, standing still.
      expect(w.retarget(room, r.x, r.y, 3.8, 1.6, now: 3), isFalse);
      expect(w.next(r.x, r.y, 3.05, stepLen: rules.step), WalkStatus.walking);
      expect((w.dx, w.dy), (0.0, 0.0));
      // It moves on: so does the hero; released, the walk ends there.
      expect(w.retarget(room, r.x, r.y, 3.5, 4.2, now: 3.1), isTrue);
      w.steering = false;
      r = run(w, room, r.x, r.y);
      expect(r.status, WalkStatus.arrived);
      expect(dist(r.x, r.y, 3.5, 4.2), lessThan(0.05));
    });

    test('gives up when it stops getting closer, unless held', () {
      final w = Walker();
      w.start(room, 1.5, 1.5, WalkGoal.ground, 4.5, 4.5, now: 0);
      // Pinned in place (a crowd): no progress.
      var status = WalkStatus.walking;
      for (var t = 0; t < 60 && status == WalkStatus.walking; t++) {
        status = w.next(1.5, 1.5, t / 20);
      }
      expect(status, WalkStatus.stuck);
      expect(w.active, isFalse);

      w
        ..start(room, 1.5, 1.5, WalkGoal.ground, 4.5, 4.5, now: 0)
        ..steering = true;
      for (var t = 0; t < 60; t++) {
        expect(w.next(1.5, 1.5, t / 20), WalkStatus.walking);
      }
    });

    test('a reconciliation that sets the hero back is no reason to give up', () {
      final w = Walker()..start(room, 1.5, 1.5, WalkGoal.ground, 4.5, 4.5, now: 0);
      // Predicted far ahead, then the server puts the hero back.
      expect(w.next(4.0, 4.0, 0), WalkStatus.walking);
      var status = WalkStatus.walking;
      var (x, y) = (1.6, 1.6);
      for (var t = 1; t < 40 && status == WalkStatus.walking; t++) {
        status = w.next(x, y, t / 20 * 3);
        (x, y) = (x + w.dx * 0.1, y + w.dy * 0.1);
      }
      expect(status, isNot(WalkStatus.stuck));
    });

    test('arrival waits a moment: a late setback walks the rest', () {
      final w = Walker()..start(room, 1.5, 1.5, WalkGoal.ground, 3.5, 1.5, now: 0);
      // Predicted on the point: standing, not done yet.
      expect(w.next(3.5, 1.5, 1), WalkStatus.walking);
      expect((w.dx, w.dy), (0.0, 0.0));
      // The server's answer puts the hero half a cell back: on again.
      expect(w.next(3.0, 1.5, 1.1), WalkStatus.walking);
      expect(w.dx, greaterThan(0));
      expect(w.next(3.5, 1.5, 1.2), WalkStatus.walking);
      expect(w.next(3.5, 1.5, 1.2 + Walker.settleSecs), WalkStatus.arrived);
      expect(w.active, isFalse);
    });

    test('no way at all: nothing starts', () {
      final shut = WalkGrid.ascii(['#####', '#.#.#', '#####']);
      final w = Walker();
      expect(w.start(shut, 1.5, 1.5, WalkGoal.npc, 3.5, 1.5, reach: 0.5, now: 0), isNull);
      expect(w.active, isFalse);
      expect(w.start(shut, 0.5, 0.5, WalkGoal.ground, 1.5, 1.5, now: 0), isNull, reason: 'standing in a wall');
    });

    test('portalAt: the trigger circle a ground point is in', () {
      const portals = [(10.5, 10.5, 3.0), (30.5, 4.5, 2.0)];
      expect(portalAt(portals, 12, 11), portals[0]);
      expect(portalAt(portals, 31, 6), portals[1]);
      expect(portalAt(portals, 20, 20), isNull);
    });
  });
}

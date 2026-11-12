import 'dart:collection';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/interpolation.dart';
import 'package:game_client/src/prediction.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/walk_grid.dart';
import 'package:test/test.dart';

/// Same 12×7 town as the server's hub tests.
final town = WalkGrid.ascii([
  '############',
  '#..........#',
  '#..........#',
  '#..........#',
  '#..........#',
  '#..........#',
  '############',
]);

void main() {
  group('protocol', () {
    test('input frame layout matches the server decoder', () {
      final b = encodeInput(7, 1, -0.5);
      expect(b, hasLength(9));
      final d = ByteData.sublistView(b);
      expect(b[0], 0x02);
      expect(d.getUint32(1, Endian.little), 7);
      expect(d.getInt16(5, Endian.little), 32767);
      expect(d.getInt16(7, Endian.little), -16384);
    });

    test('snapshot decodes and rejects truncation', () {
      final d = ByteData(13 + 9)
        ..setUint8(0, 1)
        ..setUint16(1, 3, Endian.little)
        ..setUint32(3, 1234, Endian.little)
        ..setUint32(7, 55, Endian.little)
        ..setUint16(11, 1, Endian.little)
        ..setUint32(13, 42, Endian.little)
        ..setUint16(17, (12.5 * 64).round(), Endian.little)
        ..setUint16(19, (300.25 * 64).round(), Endian.little)
        ..setUint8(21, 6);
      final b = d.buffer.asUint8List();
      final s = Snapshot.decode(b)!;
      expect(
        (s.mapEpoch, s.tick, s.ackSeq, s.ids[0], s.xs[0], s.ys[0], s.facings[0]),
        (3, 1234, 55, 42, 12.5, 300.25, 6),
      );
      expect(Snapshot.decode(Uint8List.sublistView(b, 0, 21)), isNull);
    });

    test('facing matches the server convention', () {
      expect(facingOf(0, 1), 0);
      expect(facingOf(1, 0), 2);
      expect(facingOf(0, -1), 4);
      expect(facingOf(-1, 0), 6);
      expect(facingOf(1, 1), 1);
    });
  });

  group('prediction', () {
    test('move rules slide along walls like the server', () {
      const rules = MoveRules();
      var (x, y) = (10.5, 5.5);
      for (var i = 0; i < 10; i++) {
        (x, y) = rules.apply(town, x, y, 1, 1);
      }
      expect(x, lessThan(11), reason: 'east wall at x = 11');
      expect(y, lessThan(6), reason: 'north wall at y = 6');
      expect(x, greaterThan(10.5));
    });

    test('reconciling against a lagging server leaves no error', () {
      // Server applies each input `lag` ticks after the client predicted it.
      const lag = 3;
      final p = LocalPredictor(town, 2.5, 3.5);
      var (sx, sy) = (2.5, 3.5);
      final inFlight = Queue<(int, double, double)>();
      for (var t = 0; t < 40; t++) {
        final dir = t < 25 ? (1.0, 0.3) : (0.0, 0.0);
        final seq = p.step(dir.$1, dir.$2);
        inFlight.add((seq, dir.$1, dir.$2));
        if (inFlight.length > lag) {
          final (s, dx, dy) = inFlight.removeFirst();
          (sx, sy) = const MoveRules().apply(town, sx, sy, dx, dy);
          p.reconcile(s, sx, sy);
        }
        expect(p.errX.abs() + p.errY.abs(), lessThan(1e-9), reason: 'tick $t');
      }
    });

    test('a server disagreement is smoothed, not snapped', () {
      final p = LocalPredictor(town, 2.5, 3.5);
      for (var i = 0; i < 5; i++) {
        p.step(1, 0);
      }
      final (before, _) = p.render(0.5, 0);
      // The server says we only moved four steps (it dropped one input).
      p.reconcile(5, 2.5 + 4 * 0.3, 3.5);
      final (after, _) = p.render(0.5, 0);
      expect(after, closeTo(before, 1e-9), reason: 'no visible jump at the correction');
      expect(p.x, closeTo(3.7, 1e-6));
      for (var i = 0; i < 30; i++) {
        p.render(1, 1 / 60);
      }
      expect(p.errX.abs(), lessThan(0.01), reason: 'error decays');
    });

    test('large corrections snap (teleport)', () {
      final p = LocalPredictor(town, 2.5, 3.5)..step(1, 0);
      p.reconcile(1, 9.5, 1.5);
      expect(p.render(1, 0), (9.5, 1.5));
    });
  });

  group('interpolation', () {
    test('samples between snapshots and holds after extrapolating', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 2)
        ..push(11, 0.3, 0, 2)
        ..push(12, 0.6, 0, 2);
      t.sample(11.5);
      expect(t.x, closeTo(0.45, 1e-9));
      expect(t.moving, isTrue);
      t.sample(13);
      expect(t.x, closeTo(0.9, 1e-9), reason: 'one tick of extrapolation');
      t.sample(30);
      expect(t.x, closeTo(1.2, 1e-9), reason: 'capped at two ticks');
      expect(t.moving, isFalse);
    });

    test('segment brackets the render tick and keeps the last pair past it', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 2)
        ..push(11, 0.3, 0, 2)
        ..push(12, 0.6, 0, 3);
      t.segment(11.5);
      expect((t.tick0, t.tick1, t.x0, t.x1), (11, 12, 0.3, 0.6));
      expect(t.facing, 3);
      expect(t.moving, isTrue);
      t.segment(12.5);
      expect((t.tick0, t.tick1), (11, 12), reason: 'the shader extrapolates along the last pair');
      expect(t.moving, isTrue);
      t.segment(30);
      expect(t.moving, isFalse, reason: 'stops walking once extrapolation is exhausted');
      t.segment(5);
      expect((t.x0, t.x1, t.tick0, t.tick1), (0.0, 0.0, 10, 10), reason: 'before the oldest sample: a point');
    });

    test('segment of a teleport is a point at the destination', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 0)
        ..push(11, 50, 50, 0);
      t.segment(10.5);
      expect((t.x0, t.y0, t.x1, t.y1), (50.0, 50.0, 50.0, 50.0));
      expect(t.moving, isFalse);
    });

    test('segment without samples stands at the spawn position', () {
      final t = RemoteTrack(1)
        ..x = 4
        ..y = 7;
      t.segment(0);
      expect((t.x0, t.y0, t.x1, t.y1), (4.0, 7.0, 4.0, 7.0));
    });

    test('teleports are not blended', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 0)
        ..push(11, 50, 50, 0);
      t.sample(10.5);
      expect((t.x, t.y), (50.0, 50.0));
    });

    test('render delay adapts to jitter', () {
      final calm = ServerClock(tickHz: 20);
      final jittery = ServerClock(tickHz: 20);
      final rnd = math.Random(1);
      for (var tick = 0; tick < 400; tick++) {
        calm.onSnapshot(tick, tick / 20 + 0.05);
        jittery.onSnapshot(tick, tick / 20 + 0.05 + rnd.nextDouble() * 0.06);
      }
      expect(calm.delay, closeTo(0.075, 0.01));
      expect(jittery.delay, greaterThan(calm.delay + 0.04));
      // Rendering stays behind the newest tick by about the delay.
      final now = 399 / 20 + 0.05;
      expect(399 - calm.renderTick(now), closeTo(calm.delay * 20, 0.2));
    });
  });
}

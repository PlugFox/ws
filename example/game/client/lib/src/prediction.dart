import 'dart:collection';
import 'dart:math' as math;

import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/walk_grid.dart';

/// Server movement rules, mirrored exactly (hub.rs `tick`): one step of
/// `walkSpeed / tickHz` cells per tick, axis-separated so the player slides
/// along walls; directions longer than 1 are normalised.
final class MoveRules {
  const MoveRules({this.tickHz = 20, this.walkSpeed = 6.0});

  final int tickHz;
  final double walkSpeed;

  double get step => walkSpeed / tickHz;

  /// One step; [pace] scales it (slowed: 0.5, held: 0).
  (double, double) apply(WalkGrid grid, double x, double y, double dx, double dy, [double pace = 1]) {
    final lenSq = dx * dx + dy * dy;
    if (lenSq == 0) return (x, y);
    if (lenSq > 1) {
      final len = math.sqrt(lenSq);
      dx /= len;
      dy /= len;
    }
    final d = step * pace;
    final nx = x + dx * d;
    if (grid.isWalkable(nx, y)) x = nx;
    final ny = y + dy * d;
    if (grid.isWalkable(x, ny)) y = ny;
    return (x, y);
  }
}

final class _Pending {
  _Pending(this.seq, this.dx, this.dy, this.pace);
  final int seq;
  final double dx;
  final double dy;
  final double pace;
}

/// Client-side prediction of the local player with server reconciliation.
///
/// Every fixed tick the client samples input, sends it with a new `seq` and
/// applies the same step locally. When a snapshot acknowledges `ackSeq`, the
/// authoritative position is rewound and every unacknowledged input is
/// replayed. The visual position absorbs the correction through an error
/// offset that decays over ~100 ms, so small disagreements (e.g. the server
/// applying two inputs in one tick) never show as snaps.
final class LocalPredictor {
  LocalPredictor(this.grid, this.x, this.y, {this.rules = const MoveRules()});

  WalkGrid grid;
  MoveRules rules;
  double x;
  double y;
  double prevX = 0;
  double prevY = 0;
  int facing = 4;
  bool moving = false;

  /// Step scale the server applies now (conditions): 0 = cannot move.
  double pace = 1;

  /// Visual correction still to be blended out.
  double errX = 0;
  double errY = 0;

  final Queue<_Pending> _pending = Queue();
  int _seq = 0;
  int get lastSeq => _seq;
  int get pendingCount => _pending.length;

  /// Corrections larger than this (cells) are snapped, not smoothed.
  static const double snapDistance = 3;

  void reset(WalkGrid grid, double x, double y) {
    this.grid = grid;
    this.x = prevX = x;
    this.y = prevY = y;
    errX = errY = 0;
    _pending.clear();
    moving = false;
  }

  /// Advances one fixed tick with direction ([dx], [dy]) already quantised
  /// as it goes on the wire; returns the seq to send.
  int step(double dx, double dy) {
    prevX = x;
    prevY = y;
    final seq = ++_seq;
    _pending.add(_Pending(seq, dx, dy, pace));
    // Bound memory if the server stops acknowledging.
    while (_pending.length > 64) {
      _pending.removeFirst();
    }
    final (nx, ny) = rules.apply(grid, x, y, dx, dy, pace);
    x = nx;
    y = ny;
    moving = dx != 0 || dy != 0;
    if (moving) facing = facingOf(dx, dy);
    return seq;
  }

  /// Holds the position for a tick without sending input.
  void idle() {
    prevX = x;
    prevY = y;
    moving = false;
  }

  /// Reconciles with the authoritative state from a snapshot.
  void reconcile(int ackSeq, double serverX, double serverY) {
    while (_pending.isNotEmpty && _pending.first.seq <= ackSeq) {
      _pending.removeFirst();
    }
    var rx = serverX, ry = serverY;
    for (final p in _pending) {
      (rx, ry) = rules.apply(grid, rx, ry, p.dx, p.dy, p.pace);
    }
    final ex = x - rx, ey = y - ry;
    if (ex * ex + ey * ey > snapDistance * snapDistance) {
      errX = errY = 0;
      prevX = rx;
      prevY = ry;
    } else {
      // Shift the interpolation segment with the correction and carry the
      // difference as visual error: the rendered point does not move now.
      errX += ex;
      errY += ey;
      prevX -= ex;
      prevY -= ey;
    }
    x = rx;
    y = ry;
  }

  /// Render position between the last two ticks plus the decaying error.
  (double, double) render(double alpha, double dtSec) {
    final k = math.exp(-dtSec * 12);
    errX *= k;
    errY *= k;
    if (errX.abs() < 1e-4) errX = 0;
    if (errY.abs() < 1e-4) errY = 0;
    return (prevX + (x - prevX) * alpha + errX, prevY + (y - prevY) * alpha + errY);
  }
}

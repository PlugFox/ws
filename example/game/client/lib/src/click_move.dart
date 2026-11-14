/// Click (tap) to move (COMBAT.md §4.2): a walk the player asked for by
/// clicking the ground, an NPC, a monster, a pickup bubble or a portal,
/// turned into the same move vectors the keyboard sends.
///
/// The way is found here, on the client's copy of the walk grid
/// ([WalkGrid.findPath]); the server sees nothing but ordinary inputs, so
/// prediction, reconciliation and the movement rules stay exactly as they
/// are and the client gains no new trust. Pure Dart (no browser types), so
/// it runs under `dart test`.
library;

import 'dart:math' as math;

import 'package:game_client/src/walk_grid.dart';

/// What a click asked to walk to.
enum WalkGoal {
  /// A point on the ground (or the walkable cell nearest to it).
  ground,

  /// An NPC: talk on arrival.
  npc,

  /// A monster: walk into weapon reach, then stand (the basic attack fires
  /// by itself at the target the weapon picks).
  monster,

  /// A pickup bubble: walk into it.
  bonus,

  /// A portal: walk into its trigger.
  portal,
}

/// How a tick of [Walker.next] went.
enum WalkStatus {
  /// No walk.
  idle,

  /// Under way: [Walker.dx], [Walker.dy] hold this tick's move.
  walking,

  /// In reach of the goal, or at the end of the way (the reachable point
  /// nearest to it).
  arrived,

  /// No closer for [Walker.stuckSecs]: given up (a crowd, a moved door).
  stuck,
}

/// One click-to-move walk: the goal, the way there, and the watch that
/// gives up when it stops making progress.
final class Walker {
  /// Seconds without getting closer before a walk gives up.
  static const stuckSecs = 1.5;

  /// Intermediate points count as passed this close (per axis, cells).
  static const passed = 0.35;

  /// Seconds a walk stands at its end before it is done (see [next]).
  static const settleSecs = 0.3;

  WalkGoal? goal;

  /// What the goal is about (an NPC, a monster's slot, a bonus id), for
  /// the caller.
  Object? target;

  /// The goal point and how close counts as there (cells; 0 = only the end
  /// of the way, for the ground).
  double tx = 0;
  double ty = 0;
  double reach = 0;

  /// The left button is held: the goal follows the pointer, and a walk
  /// that cannot get closer waits instead of giving up.
  bool steering = false;

  /// This tick's move (world, length ≤ 1; shorter on the last step so the
  /// hero stops on the point).
  double dx = 0;
  double dy = 0;

  List<(double, double)> _path = const [];
  int _step = 0;
  double _best = double.infinity;
  double _bestAt = 0;
  double _last = double.infinity;
  double? _settleAt;

  bool get active => goal != null;

  /// Where the way ends: the goal, or the reachable point nearest to it.
  (double, double)? get end => _path.isEmpty ? null : _path.last;

  /// Plans a walk from (px, py) to within [reach] of (tx, ty). A ground
  /// goal on a blocked cell moves to the nearest walkable one, and the way
  /// to an unreachable ground goal ends at the reachable cell nearest to
  /// it (an NPC, a monster, a bubble or a portal out of reach is no way).
  /// Returns where the way ends (for the ground marker), the start when
  /// already there, or null when there is no way at all (the walk is then
  /// not started).
  (double, double)? start(
    WalkGrid grid,
    double px,
    double py,
    WalkGoal goal,
    double tx,
    double ty, {
    double reach = 0,
    Object? target,
    required double now,
    int budget = 60000,
  }) {
    if (goal == WalkGoal.ground) {
      final snap = grid.nearestWalkable(tx, ty);
      if (snap == null) return null;
      (tx, ty) = snap;
    }
    final path = _plan(grid, px, py, tx, ty, reach, budget, closest: goal == WalkGoal.ground);
    if (path == null) return null;
    this
      ..goal = goal
      ..target = target
      ..tx = tx
      ..ty = ty
      ..reach = reach
      ..steering = false
      .._path = path
      .._step = 0
      .._best = double.infinity
      .._last = double.infinity
      .._settleAt = null
      .._bestAt = now;
    return path.isEmpty ? (px, py) : path.last;
  }

  /// Moves the goal (the pointer while steering, a chased monster) and
  /// plans again when it went more than [slack] cells from the planned
  /// one. Returns whether the way changed.
  bool retarget(
    WalkGrid grid,
    double px,
    double py,
    double tx,
    double ty, {
    required double now,
    double slack = 0.75,
    int budget = 8000,
  }) {
    final g = goal;
    if (g == null) return false;
    if (g == WalkGoal.ground) {
      final snap = grid.nearestWalkable(tx, ty);
      if (snap == null) return false;
      (tx, ty) = snap;
    }
    if ((tx - this.tx).abs() <= slack && (ty - this.ty).abs() <= slack) return false;
    final path = _plan(grid, px, py, tx, ty, reach, budget, closest: g == WalkGoal.ground);
    if (path == null) return false;
    this
      ..tx = tx
      ..ty = ty
      .._path = path
      .._step = 0
      .._best = double.infinity
      .._last = double.infinity
      .._settleAt = null
      .._bestAt = now;
    return true;
  }

  void stop() {
    goal = null;
    target = null;
    steering = false;
    dx = dy = 0;
    _path = const [];
  }

  /// This tick's move from (px, py) at [stepLen] cells per tick (the
  /// predictor's step and pace): [dx], [dy] while [WalkStatus.walking].
  /// Arriving or giving up ends the walk unless [steering].
  WalkStatus next(double px, double py, double now, {double stepLen = 0.3}) {
    dx = dy = 0;
    if (goal == null) return WalkStatus.idle;
    // Skip the points already passed (the last one must be reached).
    while (_step < _path.length - 1) {
      final (wx, wy) = _path[_step];
      if ((wx - px).abs() >= passed || (wy - py).abs() >= passed) break;
      _step++;
    }
    final (ex, ey) = _path.isEmpty ? (px, py) : _path.last;
    final left = math.sqrt((ex - px) * (ex - px) + (ey - py) * (ey - py));
    // Once there, a step off is still there: the server keeps one move per
    // tick, so a last step that shares a tick with the stop may be lost,
    // and chasing it would only repeat that (aim reaches leave a margin).
    final slack = _settleAt == null ? 0.0 : stepLen;
    if ((reach > 0 && _near(px, py, tx, ty, reach + slack)) || left < 0.05 + slack) {
      // There as predicted. Stand a moment before calling it done: a
      // reconciliation that sets the hero back (the server applied fewer
      // steps than were predicted) walks the rest. Held: stand until the
      // pointer moves on.
      if (steering) return WalkStatus.walking;
      final at = _settleAt ??= now;
      if (now - at < settleSecs) return WalkStatus.walking;
      stop();
      return WalkStatus.arrived;
    }
    _settleAt = null;
    // A reconciliation that set us back is no lack of progress either:
    // watch from here.
    if (left > _last + 0.5) _best = double.infinity;
    _last = left;
    if (left < _best - 0.25) {
      _best = left;
      _bestAt = now;
    } else if (now - _bestAt > stuckSecs) {
      if (!steering) {
        stop();
        return WalkStatus.stuck;
      }
      // Steering: wait for the pointer to go somewhere reachable.
      _bestAt = now;
      return WalkStatus.walking;
    }
    final (wx, wy) = _path[_step];
    final vx = wx - px, vy = wy - py;
    final len = math.sqrt(vx * vx + vy * vy);
    // A shorter vector is a shorter step (the server scales by its length):
    // the last one lands on the point instead of past it.
    final k = _step == _path.length - 1 && len < stepLen ? len / stepLen : 1.0;
    dx = vx / len * k;
    dy = vy / len * k;
    return WalkStatus.walking;
  }

  static List<(double, double)>? _plan(
    WalkGrid grid,
    double px,
    double py,
    double tx,
    double ty,
    double reach,
    int budget, {
    required bool closest,
  }) {
    // Open ground: straight there (no search, cheap enough every tick).
    if (grid.isWalkable(tx, ty) && grid.clearLine(px, py, tx, ty)) {
      return _near(px, py, tx, ty, reach) ? const [] : [(tx, ty)];
    }
    // Cell centres: within half a cell of an exact ground point is enough
    // for the search, then the point itself when it is in clear view.
    final path = grid.findPath(px, py, tx, ty, math.max(reach, 0.5), budget: budget, closest: closest);
    if (path == null) return null;
    if (reach == 0 && path.isNotEmpty) {
      final (lx, ly) = path.last;
      if (grid.isWalkable(tx, ty) && grid.clearLine(lx, ly, tx, ty)) {
        return [...path, (tx, ty)];
      }
    }
    return path;
  }

  static bool _near(double x, double y, double tx, double ty, double reach) {
    final dx = x - tx, dy = y - ty;
    return dx * dx + dy * dy <= reach * reach;
  }
}

/// Where a click on the ground at (x, y) leads: the portal whose trigger
/// circle holds it (`(x, y, radius)` centres, as in the manifest), or null.
(double, double, double)? portalAt(List<(double, double, double)> portals, double x, double y) {
  for (final p in portals) {
    final dx = x - p.$1, dy = y - p.$2;
    if (dx * dx + dy * dy <= p.$3 * p.$3) return p;
  }
  return null;
}

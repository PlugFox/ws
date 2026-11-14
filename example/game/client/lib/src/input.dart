/// Gamepad and touch input (COMBAT.md §4.2), reduced to what the session
/// sends each tick: a move vector, an aim, slot bits and "hold position".
///
/// This file is pure (no browser types) so it runs under `dart test`; the
/// polling of `navigator.getGamepads()` and the engine's touch overlay lives
/// in `input_devices.dart`.
library;

import 'dart:math' as math;

/// Standard-mapping gamepad buttons (W3C Gamepad, "standard" layout; Xbox
/// names: A bottom, B right, X left, Y top).
abstract final class Pad {
  static const a = 0;
  static const b = 1;
  static const x = 2;
  static const y = 3;
  static const lb = 4;
  static const rb = 5;
  static const lt = 6;
  static const rt = 7;
  static const view = 8;
  static const menu = 9;
  static const leftStick = 10;
  static const rightStick = 11;
  static const up = 12;
  static const down = 13;
  static const left = 14;
  static const right = 15;
  static const count = 17;

  /// Buttons of slots 1–5 on a page: X Y B RB, then A (the movement slot).
  static const slotButtons = [x, y, b, rb, a];
}

/// `GameView.navigateUi` actions.
abstract final class Nav {
  static const ask = -1;
  static const up = 0;
  static const down = 1;
  static const left = 2;
  static const right = 3;
  static const activate = 4;
  static const back = 5;
}

/// Radial dead zone: below [inner] nothing, then rescaled so the usable
/// throw starts at 0 and reaches 1 at [outer] (worn sticks rarely hit 1).
(double, double) radialDeadZone(double x, double y, {double inner = 0.2, double outer = 0.92}) {
  final len = math.sqrt(x * x + y * y);
  if (len <= inner || len == 0) return (0, 0);
  final m = ((len - inner) / (outer - inner)).clamp(0.0, 1.0);
  return (x / len * m, y / len * m);
}

/// A screen vector (x right, y up) as a world direction (x east, y north):
/// turned by the camera [yaw] (clockwise from north) so "up" always points
/// away from the camera, as WASD does.
(double, double) screenToWorld(double sx, double sy, double yaw) {
  final c = math.cos(yaw), s = math.sin(yaw);
  return (sx * c + sy * s, -sx * s + sy * c);
}

/// Wire aim (1/8 m, i8 per axis) of a world direction stretched to
/// [metres]; (0, 0) = no aim (auto).
(int, int) aimUnits(double wx, double wy, double metres) {
  final len = math.sqrt(wx * wx + wy * wy);
  if (len < 1e-6 || metres <= 0) return (0, 0);
  final k = metres * 8 / len;
  return ((wx * k).round().clamp(-127, 127), (wy * k).round().clamp(-127, 127));
}

/// Wire aim of an offset in cells (0.5 m: 4 aim units per cell).
(int, int) cellsToAim(double x, double y) => ((x * 4).round().clamp(-127, 127), (y * 4).round().clamp(-127, 127));

/// Auto-cast slots that hold a skill (bit i = slot i).
int autoCastSlots(int mask, List<String> hotbar) {
  var bits = 0;
  for (var i = 0; i < hotbar.length && i < 16; i++) {
    if (mask & (1 << i) != 0 && hotbar[i].isNotEmpty) bits |= 1 << i;
  }
  return bits;
}

/// What a gamepad asked for in one poll.
final class PadFrame {
  /// Left stick after the dead zone, screen space (x right, y up).
  double moveX = 0;
  double moveY = 0;

  /// Right stick after the dead zone, screen space; (0, 0) = released (auto aim).
  double aimX = 0;
  double aimY = 0;

  /// Slot bits held (bit i = slot i): X Y B RB A, LB for the second page.
  int slots = 0;

  /// RT held: stand still (keep attacking while aiming).
  bool hold = false;

  /// LB + RT + a slot button: slots whose auto-cast toggles now.
  int autoToggles = 0;

  /// Navigation actions ([Nav]) this poll: D-pad (left stick in a menu)
  /// with key repeat, and A / B while a menu has the controller.
  final nav = <int>[];

  /// Edges: A (talk / activate outside a menu), View (skill tree), Menu (settings).
  bool confirm = false;

  /// Y pressed outside a menu: in town, the nearest player's context menu.
  bool people = false;
  bool tree = false;
  bool settings = false;

  bool get aiming => aimX != 0 || aimY != 0;

  void _clear() {
    moveX = moveY = aimX = aimY = 0;
    slots = autoToggles = 0;
    hold = confirm = people = tree = settings = false;
    nav.clear();
  }
}

/// Turns raw gamepad state (pressed buttons, axes) into a [PadFrame]; keeps
/// what edges and key repeat need between polls.
final class PadMapper {
  PadMapper({this.repeatDelay = 0.4, this.repeatEvery = 0.13});

  /// Seconds before a held direction repeats, then between repeats.
  final double repeatDelay;
  final double repeatEvery;

  final _prev = List<bool>.filled(Pad.count, false);

  /// Buttons taken by the UI (A that pressed a panel button): ignored as
  /// game input until released.
  final _consumed = List<bool>.filled(Pad.count, false);
  int _navDir = -1;
  double _navNext = 0;
  final frame = PadFrame();

  /// One poll at [now] seconds. [menu]: a window (or the highlighted side
  /// panel) has the controller, so there is no game input.
  PadFrame update(List<bool> buttons, List<double> axes, double now, {required bool menu}) {
    final f = frame.._clear();
    bool down(int b) => b < buttons.length && buttons[b];
    bool edge(int b) => down(b) && !_prev[b];
    double axis(int i) => i < axes.length && axes[i].isFinite ? axes[i] : 0;
    for (var b = 0; b < Pad.count; b++) {
      if (!down(b)) _consumed[b] = false;
    }

    final (lx, ly) = radialDeadZone(axis(0), -axis(1));
    final (rx, ry) = radialDeadZone(axis(2), -axis(3));

    // Directions: the D-pad always (the panel takes it outside menus), the
    // left stick only in a menu. Repeats while held.
    var dir = down(Pad.up)
        ? Nav.up
        : down(Pad.down)
        ? Nav.down
        : down(Pad.left)
        ? Nav.left
        : down(Pad.right)
        ? Nav.right
        : -1;
    if (dir < 0 && menu && (lx * lx + ly * ly) > 0.36) {
      dir = lx.abs() > ly.abs() ? (lx > 0 ? Nav.right : Nav.left) : (ly > 0 ? Nav.up : Nav.down);
    }
    if (dir < 0) {
      _navDir = -1;
    } else if (dir != _navDir) {
      _navDir = dir;
      _navNext = now + repeatDelay;
      f.nav.add(dir);
    } else if (now >= _navNext) {
      _navNext = now + repeatEvery;
      f.nav.add(dir);
    }

    f.tree = edge(Pad.view);
    f.settings = edge(Pad.menu);
    if (menu) {
      if (edge(Pad.a)) f.nav.add(Nav.activate);
      if (edge(Pad.b)) f.nav.add(Nav.back);
      // Buttons held into the window do not fire when it closes.
      for (final b in Pad.slotButtons) {
        if (down(b)) _consumed[b] = true;
      }
    } else {
      f
        ..moveX = lx
        ..moveY = ly
        ..aimX = rx
        ..aimY = ry
        ..confirm = edge(Pad.a)
        ..people = edge(Pad.y);
      final lb = down(Pad.lb), rt = down(Pad.rt);
      if (lb && rt) {
        f.people = false;
        // Auto-cast toggles (first page); nothing casts meanwhile.
        for (var i = 0; i < Pad.slotButtons.length; i++) {
          if (edge(Pad.slotButtons[i])) f.autoToggles |= 1 << i;
        }
        for (final b in Pad.slotButtons) {
          if (down(b)) _consumed[b] = true;
        }
      } else {
        f.hold = rt;
        for (var i = 0; i < Pad.slotButtons.length; i++) {
          final b = Pad.slotButtons[i];
          if (down(b) && !_consumed[b]) f.slots |= 1 << (lb ? i + 5 : i);
        }
      }
    }
    for (var b = 0; b < Pad.count; b++) {
      _prev[b] = down(b);
    }
    return f;
  }

  /// The UI took [button] (e.g. A pressed a panel button): drop it from this
  /// frame's slots and ignore it until it is released.
  void consume(int button) {
    _consumed[button] = true;
    final i = Pad.slotButtons.indexOf(button);
    if (i >= 0) frame.slots &= ~((1 << i) | (1 << (i + 5)));
  }
}

/// The merged input of one tick: what the session reads.
final class InputState {
  /// Screen-space move (x right, y up), length ≤ 1; (0, 0) = none.
  double moveX = 0;
  double moveY = 0;

  /// Slot bits pressed by the gamepad or touch.
  int slots = 0;

  /// A manual aim (wire units) from the right stick or a touch drag.
  bool hasAim = false;
  int aimX = 0;
  int aimY = 0;

  /// Stand still (gamepad RT).
  bool hold = false;

  void clear() {
    moveX = moveY = 0;
    slots = aimX = aimY = 0;
    hasAim = hold = false;
  }

  /// Takes the larger of two move vectors (pad stick, touch joystick).
  void addMove(double x, double y) {
    if (x * x + y * y > moveX * moveX + moveY * moveY) {
      moveX = x;
      moveY = y;
    }
  }
}

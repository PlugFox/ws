import 'dart:math' as math;

import 'package:game_client/src/input.dart';
import 'package:test/test.dart';

List<bool> buttons(Iterable<int> down) {
  final b = List<bool>.filled(Pad.count, false);
  for (final i in down) {
    b[i] = true;
  }
  return b;
}

const still = [0.0, 0.0, 0.0, 0.0];

void main() {
  group('dead zone', () {
    test('drops drift and rescales the rest of the throw', () {
      expect(radialDeadZone(0.1, -0.15), (0.0, 0.0));
      expect(radialDeadZone(0.2, 0), (0.0, 0.0));
      final (x, y) = radialDeadZone(0.56, 0);
      expect(x, closeTo(0.5, 1e-9));
      expect(y, 0);
      // Full past the outer edge, direction kept.
      final (fx, fy) = radialDeadZone(0.7, 0.7);
      expect(math.sqrt(fx * fx + fy * fy), closeTo(1, 1e-9));
      expect(fx, closeTo(fy, 1e-12));
    });
  });

  group('aim', () {
    test('screen up walks away from the camera', () {
      final (x, y) = screenToWorld(0, 1, 0);
      expect(x, closeTo(0, 1e-12));
      expect(y, closeTo(1, 1e-12));
      // Camera turned 90° clockwise (looking east): up is east.
      final (ex, ey) = screenToWorld(0, 1, math.pi / 2);
      expect(ex, closeTo(1, 1e-12));
      expect(ey, closeTo(0, 1e-12));
    });

    test('direction × reach in 1/8 m, clamped to the wire', () {
      expect(aimUnits(1, 0, 4), (32, 0));
      expect(aimUnits(0, -0.3, 10), (0, -80));
      expect(aimUnits(3, 4, 5), (24, 32));
      expect(aimUnits(1, 0, 20), (127, 0));
      expect(aimUnits(0, 0, 10), (0, 0));
    });

    test('cells are 4 aim units', () {
      expect(cellsToAim(2.5, -1), (10, -4));
      expect(cellsToAim(100, 0), (127, 0));
    });
  });

  group('gamepad', () {
    test('X Y B RB A are slots 1–5; LB moves them to the second page', () {
      final pad = PadMapper();
      expect(pad.update(buttons([Pad.x, Pad.a]), still, 0, menu: false).slots, 1 | 1 << 4);
      expect(pad.update(buttons([Pad.y, Pad.b, Pad.rb]), still, 0.05, menu: false).slots, 2 | 4 | 8);
      expect(pad.update(buttons([Pad.lb, Pad.x, Pad.a]), still, 0.1, menu: false).slots, 1 << 5 | 1 << 9);
    });

    test('sticks: left moves, right aims, y is up', () {
      final pad = PadMapper();
      final f = pad.update(buttons([]), [0, -1, 1, 0], 0, menu: false);
      expect(f.moveX, closeTo(0, 1e-12));
      expect(f.moveY, closeTo(1, 1e-12));
      expect(f.aimX, closeTo(1, 1e-12));
      expect(f.aiming, isTrue);
      expect(pad.update(buttons([]), [0, 0, 0.1, 0.1], 0.05, menu: false).aiming, isFalse);
    });

    test('RT holds position; LB + RT toggles auto-cast instead of casting', () {
      final pad = PadMapper();
      final hold = pad.update(buttons([Pad.rt, Pad.x]), still, 0, menu: false);
      expect(hold.hold, isTrue);
      expect(hold.slots, 1);
      pad.update(buttons([]), still, 0.05, menu: false);
      pad.update(buttons([Pad.lb, Pad.rt]), still, 0.1, menu: false);
      final t = pad.update(buttons([Pad.lb, Pad.rt, Pad.y]), still, 0.15, menu: false);
      expect(t.autoToggles, 2);
      expect(t.slots, 0);
      // Held: no repeat toggle, and no cast when the modifiers let go.
      expect(pad.update(buttons([Pad.lb, Pad.rt, Pad.y]), still, 0.2, menu: false).autoToggles, 0);
      expect(pad.update(buttons([Pad.y]), still, 0.25, menu: false).slots, 0);
      expect(pad.update(buttons([]), still, 0.3, menu: false).slots, 0);
      expect(pad.update(buttons([Pad.y]), still, 0.35, menu: false).slots, 2);
    });

    test('menus take the D-pad, A and B with key repeat, and no game input', () {
      final pad = PadMapper(repeatDelay: 0.4, repeatEvery: 0.1);
      var f = pad.update(buttons([Pad.down, Pad.a]), [0.9, 0, 0, 0], 0, menu: true);
      expect(f.nav, [Nav.down, Nav.activate]);
      expect(f.slots, 0);
      expect(f.moveX, 0);
      expect(pad.update(buttons([Pad.down, Pad.a]), still, 0.2, menu: true).nav, isEmpty);
      expect(pad.update(buttons([Pad.down]), still, 0.41, menu: true).nav, [Nav.down]);
      expect(pad.update(buttons([Pad.down]), still, 0.45, menu: true).nav, isEmpty);
      expect(pad.update(buttons([Pad.down]), still, 0.52, menu: true).nav, [Nav.down]);
      // The left stick navigates in a menu too.
      f = pad.update(buttons([Pad.b]), [-0.9, 0, 0, 0], 0.6, menu: true);
      expect(f.nav, [Nav.left, Nav.back]);
      // A held as the window closes does not cast.
      pad.update(buttons([Pad.a]), still, 0.7, menu: true);
      expect(pad.update(buttons([Pad.a]), still, 0.75, menu: false).slots, 0);
    });

    test('View and Menu fire once per press', () {
      final pad = PadMapper();
      final f = pad.update(buttons([Pad.view, Pad.menu]), still, 0, menu: false);
      expect(f.tree, isTrue);
      expect(f.settings, isTrue);
      final g = pad.update(buttons([Pad.view, Pad.menu]), still, 0.05, menu: false);
      expect(g.tree, isFalse);
      expect(g.settings, isFalse);
    });

    test('a consumed A (a panel button) does not cast until released', () {
      final pad = PadMapper();
      final f = pad.update(buttons([Pad.a]), still, 0, menu: false);
      expect(f.confirm, isTrue);
      pad.consume(Pad.a);
      expect(f.slots, 0);
      expect(pad.update(buttons([Pad.a]), still, 0.05, menu: false).slots, 0);
      pad.update(buttons([]), still, 0.1, menu: false);
      expect(pad.update(buttons([Pad.a]), still, 0.15, menu: false).slots, 1 << 4);
    });
  });

  test('auto-cast only presses slots that hold a skill', () {
    expect(autoCastSlots(0x3FF, ['bash', '', 'provoke', '', '', '', '', '', '', 'endure']), 1 | 4 | 1 << 9);
    expect(autoCastSlots(0, ['bash']), 0);
  });

  test('the larger of two move vectors wins', () {
    final s = InputState()
      ..addMove(0.2, 0)
      ..addMove(0, -0.8)
      ..addMove(0.1, 0.1);
    expect((s.moveX, s.moveY), (0, -0.8));
  });
}

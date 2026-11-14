import 'dart:js_interop';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/engine.dart';
import 'package:game_client/src/input.dart';
import 'package:web/web.dart' as web;

/// Polls the gamepad (`navigator.getGamepads()`) and the engine's touch
/// overlay once per input tick and merges them into [state] (see
/// `input.dart` for the mapping). Menu buttons and auto-cast toggles are
/// handed back through the callbacks.
final class GameInput {
  GameInput(this.view);

  final GameView view;
  final pad = PadMapper();
  final state = InputState();

  /// Touch buffer: allocated on the JS side so `toDart` is a view, not a copy.
  final _touchJs = JSFloat32Array.withLength(8);
  late final Float32List _touch = _touchJs.toDart;

  /// The touch controls are on; a standard gamepad is connected.
  bool touchOn = false;
  bool padOn = false;
  bool _guide = false;

  /// Gamepad Menu / touch settings button.
  void Function()? onSettings;

  /// Gamepad A outside a stage and a menu (talk to the nearest NPC).
  void Function()? onInteract;

  /// Gamepad Y in town (outside a menu): the nearest player's context menu.
  void Function()? onPlayerMenu;

  /// LB + RT + slot button: toggle auto-cast of a slot.
  void Function(int slot)? onAutoToggle;

  static double _now() => web.window.performance.now() / 1000;

  /// [hotbar]: slot skill ids ('' = empty); [inStage]: on a combat stage.
  void poll({required List<String> hotbar, required bool inStage}) {
    state.clear();
    _pollTouch();
    _pollPad(hotbar, inStage);
  }

  void _pollTouch() {
    touchOn = view.readTouch(_touchJs);
    if (!touchOn) return;
    final (mx, my) = radialDeadZone(_touch[0], _touch[1], inner: 0.12, outer: 0.95);
    state.addMove(mx, my);
    final bits = _touch[4].toInt();
    state.slots |= bits;
    final ax = _touch[2], ay = _touch[3];
    if (bits != 0 && (ax != 0 || ay != 0)) {
      final (x, y) = cellsToAim(ax, ay);
      state
        ..hasAim = true
        ..aimX = x
        ..aimY = y;
    }
  }

  web.Gamepad? _gamepad() {
    final pads = web.window.navigator.getGamepads().toDart;
    for (final p in pads) {
      if (p != null && p.connected && p.mapping == 'standard') return p;
    }
    return null;
  }

  void _pollPad(List<String> hotbar, bool inStage) {
    final gp = _gamepad();
    padOn = gp != null;
    if (gp == null) {
      _hideGuide();
      return;
    }
    final buttons = [for (final b in gp.buttons.toDart) b.pressed || b.value > 0.5];
    final axes = [for (final a in gp.axes.toDart) a.toDartDouble];
    final menu = view.navigateUi(Nav.ask);
    final f = pad.update(buttons, axes, _now(), menu: menu);
    for (final a in f.nav) {
      view.navigateUi(a);
    }
    if (f.tree) view.toggleSkillTree(-1);
    if (f.settings) onSettings?.call();
    if (menu) {
      _hideGuide();
      return;
    }
    // A presses a highlighted panel button first; in town it talks.
    if (f.confirm) {
      if (view.navigateUi(Nav.activate)) {
        pad.consume(Pad.a);
      } else if (!inStage) {
        onInteract?.call();
        pad.consume(Pad.a);
      }
    }
    // Y in town (skills are not cast there): the player menu.
    if (f.people && !inStage && onPlayerMenu != null) {
      onPlayerMenu!();
      pad.consume(Pad.y);
    }
    for (var i = 0; i < 5; i++) {
      if (f.autoToggles & (1 << i) != 0) onAutoToggle?.call(i);
    }
    state
      ..addMove(f.moveX, f.moveY)
      ..slots |= f.slots
      ..hold = f.hold;
    if (!f.aiming) {
      _hideGuide();
      return;
    }
    // Right stick: direction × the reach of the pressed skill (the longest
    // of the page while nothing is pressed, for the guide).
    String at(int i) => i < hotbar.length ? hotbar[i] : '';
    var reach = 0.0;
    for (var i = 0; i < 10; i++) {
      if (f.slots & (1 << i) != 0 && at(i).isNotEmpty) reach = math.max(reach, view.skillReach(at(i)));
    }
    if (reach == 0) {
      final page = buttons.length > Pad.lb && buttons[Pad.lb] ? 5 : 0;
      for (var i = page; i < page + 5; i++) {
        if (at(i).isNotEmpty) reach = math.max(reach, view.skillReach(at(i)));
      }
    }
    if (reach == 0) reach = 3;
    final (wx, wy) = screenToWorld(f.aimX, f.aimY, view.cameraYaw());
    final len = math.sqrt(wx * wx + wy * wy);
    // Cells are 0.5 m.
    view.setAimGuide(wx / len * reach * 2, wy / len * reach * 2, reach * 2);
    _guide = true;
    if (f.slots != 0) {
      final (x, y) = aimUnits(wx, wy, reach);
      state
        ..hasAim = true
        ..aimX = x
        ..aimY = y;
    }
  }

  void _hideGuide() {
    if (!_guide) return;
    _guide = false;
    view.setAimGuide(double.nan, double.nan, 0);
  }
}

import 'dart:js_interop';
import 'dart:typed_data';

import 'package:web/web.dart' as web;

/// Bindings to the TypeScript `GameView` façade (`engine.bundle.js` sets
/// `window.GameEngine`). This is the whole surface: no Three.js types leak
/// into Dart.
@JS('GameEngine')
external GameEngineModule get gameEngine;

extension type GameEngineModule._(JSObject _) implements JSObject {
  external GameView createGameView();
  // ignore: non_constant_identifier_names
  external int get ENTITY_STRIDE;
}

extension type GameView._(JSObject _) implements JSObject {
  external JSPromise<JSAny?> init(web.HTMLCanvasElement canvas);
  external JSPromise<JSAny?> loadLocation(String manifestUrl);
  external void setCamera(String constraintJson);
  external double cameraYaw();

  /// Light radius of the focused unit in cells (0 = no limit): beyond it
  /// the world sinks into fog and [Row.flagFade] units dissolve.
  external void setLightRadius(double cells);
  external int spriteIndex(String name);
  external int actionIndex(String name);
  @JS('upsertEntities')
  external void _upsertEntities(JSFloat32Array rows, int count);
  external void removeEntity(double id);
  external void clearEntities();

  /// Draws a frame at [renderTime] (seconds on the same clock as the
  /// upserted segments' t0/t1).
  external void renderFrame(double renderTime);
  external void setLabel(double entityId, String? text, String kind);
  external double pickEntity(double clientX, double clientY);
  @JS('pickGround')
  external bool _pickGround(double clientX, double clientY, JSFloat32Array out);
  external void showDialog(String specJson);
  external void showSettings(String specJson);

  /// Non-modal side panel (`PanelSpec` JSON); an empty string hides it.
  external void showPanel(String specJson);

  /// Applies a partial settings JSON; returns the effective settings JSON.
  external String configure(String settingsJson);
  external void closeDialog();
  external bool dialogOpen();
  external void onUiEvent(JSFunction callback);
  external void setHint(String? text);
  external void toast(String text);
  external void updateHud(String text);
  external void playBgm(String? url);
  external String stats();

  /// Loads the character customization pack (dyeable bodies and hair).
  /// Call once, before the first `loadLocation`.
  external JSPromise<JSAny?> loadCustomization(String baseUrl);

  /// Sprite index of the dyeable body of a class and gender (0 male,
  /// 1 female), or -1 without the customization pack. [job] is a body slug
  /// (`novice`, `swordsman`, `crusader`, `wizard`, ...; see `bodyJob`);
  /// an unknown one is the novice. The clothes dyes follow the body drawn.
  external int bodySpriteIndex(int gender, String job);

  /// Hair style and dyes of a character drawn with [bodySpriteIndex];
  /// a negative gender removes them.
  external void setLook(double entityId, int gender, int hair, int hairColor, int bodyColor);

  /// Health bar under a player's name on combat maps (0..1; NaN hides it).
  external void setHealth(double entityId, double fraction);

  /// Floating combat number (`flags` as in [Hit]); [hurt] = the target is a player.
  external void showDamage(double entityId, int amount, int flags, bool hurt);

  /// The local player's bars as JSON (see the engine); '' hides them.
  external void setStatus(String json);

  /// Full-screen character slots and creation (`CharacterSelectSpec` JSON).
  /// Events: `char_select`, `char_create`.
  external void showCharacterSelect(String specJson);

  /// Stylist window (`StylistSpec` JSON). Events: `style_apply`, `style_cancel`.
  external void showStylist(String specJson);

  /// Closes the character select or stylist screen.
  external void closeScreen();

  /// Skill table and icons for the hotbar and the tree (`<baseUrl>/tree.json`).
  external JSPromise<JSAny?> loadSkills(String baseUrl);

  /// Own skills JSON `{points, levels, hotbar, level, cls, keys}`; '' hides
  /// the hotbar. Events: `skill_learn`, `skill_slot`, `skill_tree`.
  external void setSkills(String json);

  /// A cast starts (telegraphs; `own` also drives the cast bar and
  /// cooldown): `{caster, skill, x, y, castMs, cooldownMs, level, own,
  /// areas: [{id, shape, look, x, y, dir, a, b, spanMs, follow}]}`.
  external void castSkill(String json);

  /// Lasting ground `id` ended early (a spent ward, a sprung trap).
  external void endZone(int id);

  /// The boss's bar at the top (`{id, name, hp, maxHp}` JSON; '' hides it).
  external void setBoss(String json);

  /// Own buff icons (`[{skill, ms}]` JSON; ms 0 = until toggled off).
  external void setBuffs(String json);

  /// Opens (1), closes (0) or toggles (-1) the skill tree; returns whether it is open.
  external bool toggleSkillTree(int mode);

  /// Touch controls: -1 auto (touch devices), 0 off, 1 on. Events:
  /// `skill_auto {slot}` (long press), `open_settings`.
  external void setTouchControls(int mode);

  /// Touch state for this tick into [out] (6 floats: move x, y; aim x, y in
  /// cells; slot bits released since the last read; 1 while aiming). Pass a
  /// JS-allocated array and read it through `toDart` (no copy under dart2js
  /// or dart2wasm). False (zeroed) when the touch controls are off.
  external bool readTouch(JSFloat32Array out);

  /// Auto-cast slots (bit i = slot i), marked on the hotbar and touch buttons.
  external void setAutoCast(int mask);

  /// Metres a manual aim reaches for [skill] (its range, or a short default).
  external double skillReach(String skill);

  /// Aim guide around the local player: offset and reach in cells; NaN hides.
  external void setAimGuide(double x, double y, double reach);

  /// Controller navigation of the open window or side panel (`Nav` codes);
  /// true when the UI took it, false when it is game input.
  external bool navigateUi(int action);

  /// Emote sprites (`<baseUrl>/emotes.json`); without them emotes show nothing.
  external JSPromise<JSAny?> loadEmotes(String baseUrl);

  /// `[{id, name}]` JSON of the loaded emotes.
  external String emoteNames();

  /// Names of the ten flag kinds (JSON array).
  external String flagNames();

  /// Plays emote [emote] over an entity's head for a moment.
  external void showEmote(double entityId, int emote);

  /// A ping flag of kind [flag] (0..9) on cell ([x], [y]) for a few seconds,
  /// [label] under it; a newer flag of the same owner replaces it.
  external void showFlag(double ownerId, int flag, double x, double y, String label);

  /// Passes the rows as a view on the same buffer: under dart2js and
  /// dart2wasm (with a JS-backed list) this crosses without a copy.
  void upsertEntities(Float32List rows, int count) {
    if (count > 0) _upsertEntities(rows.toJS, count);
  }

  /// Ground cell under a client-space point, or null.
  (double, double)? pickGround(double clientX, double clientY) {
    final out = Float32List(2);
    return _pickGround(clientX, clientY, out.toJS) ? (out[0].toDouble(), out[1].toDouble()) : null;
  }
}

/// Shared action ids (engine `ACTIONS`).
abstract final class Act {
  static const idle = 0;
  static const walk = 1;
  static const sit = 2;
  static const attack = 3;
  static const hit = 4;
  static const die = 5;
}

/// Entity row layout for `upsertEntities` (engine `units.ts`):
/// `id, x0, y0, t0, x1, y1, t1, facing, sprite, action, speed, flags`.
abstract final class Row {
  static const stride = 12;

  /// The camera follows this entity.
  static const flagFocus = 1;

  /// Players and bosses (high-rate tier).
  static const flagHigh = 2;

  /// Monsters: dissolve in and out at the edge of the light radius.
  static const flagFade = 4;

  /// Condition tints (engine `units.ts`): ice, stone, stars, slow, blind,
  /// fire, poison, rage.
  static const flagFrozen = 8;
  static const flagPetrified = 16;
  static const flagStunned = 32;
  static const flagSlowed = 64;
  static const flagBlinded = 128;
  static const flagBurning = 256;
  static const flagPoisoned = 512;
  static const flagTaunted = 1024;
}

/// Growable row buffer for `upsertEntities`; reused every frame.
final class RowBuffer {
  Float32List rows = Float32List(64 * Row.stride);
  int length = 0;

  void add({
    required double id,
    required double x0,
    required double y0,
    required double t0,
    required double x1,
    required double y1,
    required double t1,
    required int facing,
    required int sprite,
    required int action,
    double speed = 1,
    int flags = 0,
  }) {
    if ((length + 1) * Row.stride > rows.length) {
      rows = Float32List(rows.length * 2)..setAll(0, rows);
    }
    final o = length++ * Row.stride;
    rows[o] = id;
    rows[o + 1] = x0;
    rows[o + 2] = y0;
    rows[o + 3] = t0;
    rows[o + 4] = x1;
    rows[o + 5] = y1;
    rows[o + 6] = t1;
    rows[o + 7] = facing.toDouble();
    rows[o + 8] = sprite.toDouble();
    rows[o + 9] = action.toDouble();
    rows[o + 10] = speed;
    rows[o + 11] = flags.toDouble();
  }

  /// Sends the pending rows and empties the buffer.
  void flush(GameView view) {
    view.upsertEntities(rows, length);
    length = 0;
  }
}

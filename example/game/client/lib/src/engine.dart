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

  /// The loading screen: `{title, subtitle?, map?, kind?, tips?, now?}`
  /// when a location starts loading, [endLoading] once it is playable.
  external void beginLoading(String specJson);
  external void endLoading();
  external void setCamera(String constraintJson);
  external double cameraYaw();

  /// Light radius of the focused unit in cells (0 = no limit): beyond it
  /// the world sinks into fog and [Row.flagFade] units dissolve.
  external void setLightRadius(double cells);

  /// Camera shake, [strength] 0..1 over [ms] (skipped with reduced motion).
  external void shakeCamera(double strength, int ms);

  /// A map event for the engine's weather effects (`weather.ts`):
  /// `{phase, id, name, startMs, durationMs}`.
  external void setEnvironmentEvent(String json);
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

  /// Pickup bubble (`showBonus` id) under a client-space point, or NaN.
  external double pickBonus(double clientX, double clientY);
  @JS('pickGround')
  external bool _pickGround(double clientX, double clientY, JSFloat32Array out);
  external void showDialog(String specJson);
  external void showSettings(String specJson);

  /// The party panel (`PanelSpec` JSON): the Social window's Party tab
  /// (a new `id` opens it unless `quiet`); an empty string clears it.
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
  external bool toggleMute();
  external bool isMuted();
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

  /// Health bar under a player's name on combat maps, or under an elite
  /// or boss (0..1; NaN hides it). Kept until the entity's plate shows.
  external void setHealth(double entityId, double fraction);

  /// Mana bar under a player's health bar on combat maps (0..1; NaN hides it).
  external void setMana(double entityId, double fraction);

  /// Revive ring over a downed player, filling while an ally stands by
  /// (0..1; NaN removes it).
  external void setRevive(double entityId, double fraction);

  /// Floating combat number (`flags` as in [Hit]); [hurt] = the target is a player.
  external void showDamage(double entityId, int amount, int flags, bool hurt);

  /// A drop floating up from the monster [entityId] (cell [x], [y] once it
  /// is gone): [itemId]'s icon and name, or the amount for `zeny`, with a
  /// soft pickup sound; [rare] drops glow.
  external void showLoot(double entityId, double x, double y, String itemId, int count, bool rare);

  /// A pickup bonus [bonus] (`storm_gust`, `zeny`, ...) bobbing in its
  /// bubble over cell ([x], [y]), marked on the minimap; it blinks before
  /// bursting in [lifeMs].
  external void showBonus(double id, String bonus, double x, double y, int lifeMs);

  /// Pickup bonus [id] is gone: taken by [entityId] (a burst, its sound and
  /// name over them), or burst untouched when [entityId] is 0.
  external void takeBonus(double id, double entityId);

  /// The overtime poison fog (`PoisonFog.toJson` plus `warn`): the engine
  /// shrinks its safe circle from the run clock, draws the wall, the haze
  /// and the minimap ring, and cues the player inside it; `warn` sounds
  /// the alarm. An empty string clears it.
  external void setPoisonFog(String json);

  /// The local player's net seconds inside the fog (0 hides the meter).
  external void setFogExposure(int secs, int deathSecs);

  /// Weapon family a character holds (`sword`, `bow`, ...): its attack sound.
  external void setWeapon(double entityId, String kind);

  /// Headgear a character wears: item ids, comma separated ('' = none).
  external void setHeadgear(double entityId, String items);

  /// A unit's own sound: `attack`, `hit`, `die`, `move`.
  external void unitSound(double entityId, String event);

  /// Level-up burst (the angel and the chime) over a character.
  external void levelUp(double entityId);

  /// A ranged basic attack flies from one unit to another ([kind]: `arrow`);
  /// a monster's blow from next to its target shows nothing.
  external void projectile(double fromId, double toId, String kind);

  /// The local player's bars as JSON (see the engine); '' hides them.
  external void setStatus(String json);

  /// Full-screen character slots and creation (`CharacterSelectSpec` JSON).
  /// Events: `char_select`, `char_create`.
  external void showCharacterSelect(String specJson);

  /// The feedback form (`FeedbackSpec` JSON: `{kind, priority, kinds,
  /// priorities, titleMin, titleMax, bodyMin, bodyMax, version?, busy?,
  /// error?}`); '' closes it. Events: `feedback_submit {kind, priority,
  /// title, body}`, `feedback_close`.
  external void showFeedback(String specJson);

  /// The mission result screen (`ResultsSpec` JSON: the party's run
  /// statistics, the player's rewards and loot); '' closes it. Event:
  /// `results_close`.
  external void showResults(String specJson);

  /// Settings → Account: the account's optional contacts (`AccountSpec`
  /// JSON: `{username, email?, emailVerified?, emailGoogle?, telegram?,
  /// telegramVerified?, busy?, error?, field?, note?}`); '' closes it.
  /// Events: `account_save {email, telegram}`, `account_close`.
  external void showAccount(String specJson);

  /// The version line (client and server) in the title screen corner.
  external void setVersion(String text);

  /// Stylist window (`StylistSpec` JSON). Events: `style_apply`, `style_cancel`.
  external void showStylist(String specJson);

  /// Closes the sign-in, character select or stylist screen.
  external void closeScreen();

  /// Loads the interface atlas (`client/ui/`: one texture for every window,
  /// button and gauge, plus the title backdrop).
  external JSPromise<JSAny?> loadUi(String baseUrl);

  /// The sign-in window and its "Create account" form over the title
  /// backdrop (`{user?, error?, field?, busy?, remember?, mode?}` JSON).
  /// Events: `login {user, password, create, remember, email?, telegram?}`.
  external void showLogin(String specJson);

  /// Opens / closes an interface window (`equip`, `skills`, `map`,
  /// `basic`, `social` and its tabs `party`, `friends`, …); returns
  /// whether it is open now.
  external bool toggleWindow(String id);

  /// Debug overlay data (`{map, channel, transport, players, monsters,
  /// rttMs, jitterMs, interpMs, snapshotsPerSec, kbInPerSec, kbOutPerSec,
  /// pendingInputs, x, y}` JSON; any subset).
  external void setNetStats(String json);

  /// Debug overlay mode (0 off … 6 all); [cycleDebugMode] steps it (F1)
  /// and returns the new one.
  external void setDebugMode(int mode);
  external int cycleDebugMode();

  /// Leaves the world: clears entities, labels, windows and the HUD (the
  /// location stays drawn under the title screen).
  external void resetSession();

  /// Full screen on / off; returns whether it was asked to go full screen.
  external bool toggleFullscreen();

  /// Minimap marks (`[{id, kind: 'party' | 'boss'}]` JSON); they follow the
  /// entities while drawn. '' clears them.
  external void setMinimapMarks(String json);

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

  /// Scripted elite / boss encounters (`BOSS_*`): a cast starts
  /// (`{caster, ability, name, castMs, channel, shielded, interrupted}`;
  /// an empty `ability` ends it early): the cast bar under its encounter
  /// bar and the ability's cast effects.
  external void bossCast(String json);

  /// The ability's danger (and safe) zones: `{caster, ability, zones: [{id,
  /// shape (3 = ring: a outer, b inner), look, x, y, dir, a, b, spanMs,
  /// follow, delayMs, safe}]}`; each fills over its own delay.
  external void bossZones(String json);

  /// A boss yell: a styled bubble over [entityId] and a log line
  /// ([style] 0 elite, 1 boss).
  external void bossShout(double entityId, String name, String text, int style);

  /// An encounter bar at the top (`{id, name, rank, hp, maxHp, shield,
  /// phase, pips, enraged}`; rank 0 drops that bar; '' drops them all).
  external void setBossBar(String json);

  /// The monster affix table (server `AFFIX_DEFS`): `{defs: [{id, bit,
  /// name, desc, color}], ranks, expPct, dropPct, extraPct, hpPct}`.
  external void setAffixDefs(String json);

  /// A monster's affixes: bit mask (0 clears), state (1 frenzied, 2 shield
  /// broken, 4 minion) and its display name; the engine draws its stars,
  /// plate, auras and boss-bar line.
  external void setAffixes(double entityId, int bits, int state, String name);

  /// A moment of an affix trick (`blink_mark`, `blink`, `summon`, `split`,
  /// `frenzy`, `thorns`, `fuse`) at a cell position.
  external void affixEvent(double entityId, String name, double x, double y);

  /// Own buff icons (`[{skill, ms}]` JSON; ms 0 = until toggled off).
  external void setBuffs(String json);

  /// The own kill combo: [count] kills at style [rank] (0 none, 1 D …
  /// 7 SSS); the bar drains [leftMs] of [windowMs]. A higher rank pops in
  /// with the announcer's line; [count] 0 fades the meter out.
  external void setCombo(int count, int rank, int windowMs, int leftMs);

  /// A mission brief (`MissionBrief.toJson()`): the mission window and the
  /// run's timeline draw only from it.
  external void setMissionBrief(String json);

  /// The mission window (`{name, missions: [{id, name, description,
  /// maxPlayers}], inLobby?, selected?}`; '' closes it). Events:
  /// `mission_select {mission, public}`, `party_code {code}`,
  /// `missions_close`.
  external void showMissions(String json);

  /// The mission modifier table (server `MODIFIER_DEFS`): `{defs: [{id, bit
  /// (255 = the difficulty slider), name, desc, icon, risk, minLevel,
  /// locked, steps: [{label, detail}]}], rewards: [{zenyPct, expPct,
  /// chest}]}` (the clear bonus by risk). The mission window draws its
  /// Modifiers section only from it.
  external void setModifierDefs(String json);

  /// A modifier choice: `{scope: 'lobby', mission?, leader?, state?}` (the
  /// party's, editable by the leader in the mission window) or `{scope:
  /// 'stage', state?}` (the run's: icons by the HUD timeline); `state` =
  /// `{tier, mask, risk, zenyPct, expPct, chest, lootPct}`, absent clears.
  /// Events: `mission_modifiers {tier, mask}` (the leader's change);
  /// `mission_select` carries `{tier, mask}` chosen before a party exists.
  external void setMissionModifiers(String json);

  /// The run's timeline on the HUD for [mission] ('' hides it), [elapsedMs]
  /// into the run, counting on while [running].
  external void setMissionClock(String mission, int elapsedMs, bool running);

  /// The item catalogue: `{icons, items: [{id, name, kind, slot, price,
  /// classes, desc}]}`.
  external void setItems(String json);

  /// The own belongings: `{zeny, items: [{id, count}], equip: [10 ids], cls,
  /// combat}`; ids are instance keys (`sword+7[card]`, one per stack).
  /// Events: `item_use {item}`, `item_equip {item}`,
  /// `item_unequip {slot}`, `quick_potion {kind, item}` (orb click).
  external void setInventory(String json);

  /// A merchant's window `{npc, name, items}`; '' closes it. Events:
  /// `shop_deal {npc, buy: [{id, count}], sell: [{id, count}]}` (the
  /// cart), `shop_close`.
  external void showShop(String json);

  /// A line in the open shop's footer.
  external void shopNote(String text);

  /// The cart went through ([spent] and [earned] zeny): the shop empties it.
  external void shopDone(int spent, int earned);

  /// Player trade: `{request: {name, level}}`, `{pending: {name}}`, the
  /// table `{partner, mine, theirs, myLock, theirLock, myOk, theirOk}`,
  /// `{result: 'done'}` or '' (closes). Events: `trade_respond {accept}`,
  /// `trade_cancel`, `trade_item {item, count}`, `trade_zeny {zeny}`,
  /// `trade_lock`, `trade_confirm`.
  external void showTrade(String json);

  /// The context menu on another player: `{target, name, title?, x?, y?,
  /// items: [{id, label, detail?, disabled?}]}` (x / y in client px, else
  /// next to the player); '' closes it. Event: `player_action {action,
  /// target}`.
  external void showPlayerMenu(String json);

  /// The player the open context menu is for (0 = closed).
  external double playerMenuTarget();

  /// Another player's gear, read-only: `{id, name, title?, level?, look?,
  /// equip: [10 ids]}`; '' closes it.
  external void showInspect(String json);

  /// The entity under the pointer (NaN = none): its name lights up.
  external void setHover(double entityId, bool clickable);

  /// The cursor over the game world: `talk` (an NPC), `attack` (a
  /// monster), `pick` (loot), `walk` (open ground, click to move),
  /// `portal`, `wait`, `disabled`; '' the default arrow.
  external void setCursor(String kind);

  /// Click to move: the ground marker at cell ([x], [y]) where the walk
  /// ends, a ring that closes and fades; [kind] 1 = it stops short of the
  /// click (nothing reachable nearer). NaN removes it.
  external void markMove(double x, double y, int kind);

  /// The potion the `hp` / `sp` orb would drink now ('' = none).
  external String quickPotion(String kind);

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
  /// With [skill] (the one being aimed) it shows that skill's areas.
  external void setAimGuide(double x, double y, double reach, [String skill]);

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

  /// Local chat on (in game: the chat log window, Enter opens its input) or
  /// off. Sent lines come back as the `chat_send` UI event `{text}`.
  external void setChat(bool on);

  /// A player's chat line: a bubble over [entityId] (`Name : text`, fades
  /// after a few seconds) and a log line; [self] = the local player.
  external void chatLine(double entityId, String name, String text, bool self);

  /// A server announcement: a banner at the top and a chat log line.
  external void announce(String text);

  /// A grey chat log line.
  external void chatNotice(String text);

  /// Opens the chat log with its input focused (gamepad menu, touch).
  external void openChat();

  /// The friends list (`{friends: [{account, link: 'friend' | 'incoming' |
  /// 'outgoing', name, online, cls, level, map, channel, chars}]}` JSON):
  /// the Friends window (`toggleWindow('friends')`, Alt+H), the badge on
  /// its Basic Info button (requests waiting) and [isFriend]. Events:
  /// `friend_action {op: accept | decline | remove | add_name, account?,
  /// name?}`.
  external void setFriends(String json);

  /// The player [entityId] (a character id) is a friend or asked / was
  /// asked: the player menu leaves out "Add friend".
  external bool isFriend(double entityId);

  /// A leaderboard page (`{sort, page, pages, total, entries: [{rank, id,
  /// name, class, title, level, kills, mvps, zeny, deaths, missions,
  /// best_combo}], me?, online?, loading?, error?}` JSON, the server's
  /// `/api/leaderboard` answer plus `online`): the Social window's Ranking
  /// tab (`toggleWindow('leaderboard')`). Events: `leaderboard_query {sort,
  /// page, limit}` when the tab is shown, a sort is picked or a page turns.
  external void setLeaderboard(String json);

  /// Players online and the poll's round trip on the title screens
  /// (`{online, pingMs?, down?}` JSON); '' removes it (so does
  /// [closeScreen]).
  external void setServerStatus(String json);

  /// The connection to the game server is lost and being retried
  /// (`{attempt, text?}` JSON): a small plate with a spinner over the
  /// world; '' removes it (so does [resetSession]).
  external void setConnection(String json);

  /// Players online, beside the minimap (town); 0 hides it.
  external void setOnline(int count);

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

  /// Combat stance (weapon drawn).
  static const ready = 6;

  /// Swing with a weapon (the engine plays whichever of the two the class
  /// and weapon family draw the weapon in).
  static const attack2 = 7;
  static const attack3 = 8;

  /// The skill wind-up pose (held on its last frame).
  static const cast = 9;
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

  /// Burrowed: drawn as a moving mound of earth instead of the sprite
  /// (2048 is the engine's own leap bit).
  static const flagBurrowed = 4096;
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

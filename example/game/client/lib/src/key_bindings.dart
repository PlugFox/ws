/// Rebindable keys: hotbar slots, emotes, flags and a few actions. A
/// binding is `Ctrl+` / `Alt+` / `Shift+` prefixes and a
/// `KeyboardEvent.code` (`Alt+Digit1`); '' leaves the action unbound. The
/// engine's settings window edits them (field type `key`); they persist
/// with the other settings under `bind.*`.
library;

import 'package:web/web.dart' as web;

/// One key with its modifiers; the modifiers must match exactly, so `1`
/// casts slot 1 while `Alt+1` plays an emote.
final class KeyBinding {
  const KeyBinding(this.code, {this.ctrl = false, this.alt = false, this.shift = false});

  /// Null for '' (unbound) or a malformed string.
  static KeyBinding? parse(String s) {
    if (s.isEmpty) return null;
    final parts = s.split('+');
    final code = parts.last;
    if (code.isEmpty) return null;
    final mods = parts.sublist(0, parts.length - 1);
    return KeyBinding(code, ctrl: mods.contains('Ctrl'), alt: mods.contains('Alt'), shift: mods.contains('Shift'));
  }

  final String code;
  final bool ctrl;
  final bool alt;
  final bool shift;

  bool matches(web.KeyboardEvent e) =>
      e.code == code && e.ctrlKey == ctrl && e.altKey == alt && e.shiftKey == shift;

  /// Held right now, from the set of pressed codes.
  bool heldIn(Set<String> keys) {
    bool any(String a, String b) => keys.contains(a) || keys.contains(b);
    return keys.contains(code) &&
        any('ControlLeft', 'ControlRight') == ctrl &&
        any('AltLeft', 'AltRight') == alt &&
        any('ShiftLeft', 'ShiftRight') == shift;
  }

  /// Short caption for the hotbar (`Alt+1`, `Q`, `␣`).
  String get caption {
    final key = code.startsWith('Digit')
        ? code.substring(5)
        : code.startsWith('Key')
        ? code.substring(3)
        : code.startsWith('Numpad')
        ? 'Num${code.substring(6)}'
        : code == 'Space'
        ? '␣'
        : code.replaceFirst('Arrow', '');
    return '${ctrl ? 'Ctrl+' : ''}${alt ? 'Alt+' : ''}${shift ? '⇧' : ''}$key';
  }
}

/// Every binding of the game, read from the settings map.
final class Bindings {
  Bindings(Map<String, Object?> settings) {
    String get(String id) => (settings[id] as String?) ?? defaults[id] ?? '';
    for (var i = 0; i < count; i++) {
      slots[i] = KeyBinding.parse(get('bind.slot.$i'));
      emotes[i] = KeyBinding.parse(get('bind.emote.$i'));
      flags[i] = KeyBinding.parse(get('bind.flag.$i'));
      emoteIds[i] = (settings['emote.$i'] as num?)?.toInt() ?? defaultEmotes[i];
    }
    interact = KeyBinding.parse(get('bind.interact'));
    interact2 = KeyBinding.parse(get('bind.interact2'));
    travel = KeyBinding.parse(get('bind.travel'));
    items = KeyBinding.parse(get('bind.items'));
    potion = KeyBinding.parse(get('bind.potion'));
    potion2 = KeyBinding.parse(get('bind.potion2'));
    sit = KeyBinding.parse(get('bind.sit'));
    equip = KeyBinding.parse(get('bind.equip'));
    info = KeyBinding.parse(get('bind.info'));
    party = KeyBinding.parse(get('bind.party'));
    friends = KeyBinding.parse(get('bind.friends'));
    leaderboard = KeyBinding.parse(get('bind.leaderboard'));
    debug = KeyBinding.parse(get('bind.debug'));
  }

  /// Ten of each: hotbar slots, emote keys and flag kinds.
  static const count = 10;

  /// Emotes of the ten emote keys by default (RO emotion ids): surprise,
  /// question, delight, heart, thanks, sorry, smile, ok, go, congratulations.
  static const defaultEmotes = [0, 1, 2, 3, 15, 17, 18, 33, 27, 46];

  static String _digit(int i) => 'Digit${(i + 1) % 10}';

  static final defaults = <String, String>{
    for (var i = 0; i < count; i++) ...{
      'bind.slot.$i': _digit(i),
      'bind.emote.$i': 'Alt+${_digit(i)}',
      'bind.flag.$i': 'Ctrl+${_digit(i)}',
    },
    'bind.interact': 'KeyF',
    'bind.interact2': 'Space',
    'bind.travel': 'Space',
    'bind.items': 'Alt+KeyE',
    // Q / E turn the camera; R and T drink (missions).
    'bind.potion': 'KeyR',
    'bind.potion2': 'KeyT',
    'bind.sit': 'KeyC',
    'bind.equip': 'Alt+KeyQ',
    'bind.info': 'Alt+KeyV',
    // RO's party window key.
    'bind.party': 'Alt+KeyZ',
    // RO's friends window key.
    'bind.friends': 'Alt+KeyH',
    // L for ladder: the Social window's Ranking tab.
    'bind.leaderboard': 'Alt+KeyL',
    'bind.debug': 'F1',
  };

  final slots = List<KeyBinding?>.filled(count, null);
  final emotes = List<KeyBinding?>.filled(count, null);
  final flags = List<KeyBinding?>.filled(count, null);
  final emoteIds = List<int>.filled(count, 0);
  KeyBinding? interact;
  KeyBinding? interact2;

  /// On missions: the learned travel skill (Back Slide, Blink, Charge, ...).
  KeyBinding? travel;
  KeyBinding? items;

  /// Drink the best healing / SP potion (missions).
  KeyBinding? potion;
  KeyBinding? potion2;
  KeyBinding? sit;
  KeyBinding? equip;
  KeyBinding? info;

  /// The party window (lobby, stage members, or how to find a party).
  KeyBinding? party;

  /// The Friends window.
  KeyBinding? friends;

  /// The leaderboard (Social window, Ranking tab).
  KeyBinding? leaderboard;
  KeyBinding? debug;

  List<String> get slotCaptions => [for (final b in slots) b?.caption ?? ''];

  /// Index of the emote / flag key [e] presses, or -1.
  int emoteOf(web.KeyboardEvent e) => emotes.indexWhere((b) => b != null && b.matches(e));
  int flagOf(web.KeyboardEvent e) => flags.indexWhere((b) => b != null && b.matches(e));
}

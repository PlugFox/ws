/// Refining at a smith: the client's copy of the server's table
/// (`combat/refine.rs`), so the smith's window can show the ore, the fee
/// and the odds before the player asks. The server decides; this only
/// shows. Change both together (`client_test.dart`, `refine table`).
library;

import 'protocol.dart';

const phracon = 'phracon';
const emveretarcon = 'emveretarcon';
const oridecon = 'oridecon';
const elunium = 'elunium';

/// Highest refine level.
const maxRefine = 10;

/// Odds of an attempt past the safe limit, by how far past (1, 2, …).
const _pastSafe = [70, 55, 40, 30, 20, 10];

/// Fee of the attempt that reaches +1 … +10.
const _fees = [50, 100, 200, 400, 1000, 2000, 4000, 10000, 20000, 40000];

/// One attempt: the level it reaches, its ore, fee, odds (percent) and the
/// die of levels a failure loses ([slip]: d2 … d8).
typedef RefineStep = ({int to, String ore, int fee, int chance, int slip});

/// Sides of the die a failure from level [from] loses levels by: the
/// higher the item, the bigger the die (a failed +9 can fall to +1).
int slipDie(int from) => from <= 5
    ? 2
    : from <= 7
    ? 4
    : from == 8
    ? 6
    : 8;

/// The attempt from level [from] on [def], or null (not refinable, +10).
RefineStep? refineStep(ItemDef def, int from) {
  if (!def.refinable || from >= maxRefine) return null;
  final to = from + 1;
  final ore = to <= 4
      ? phracon
      : to <= 7
      ? emveretarcon
      : def.kind == 2
      ? oridecon
      : elunium;
  final safe = def.refineSafe;
  final chance = to <= safe ? 100 : _pastSafe[to - safe - 1];
  return (to: to, ore: ore, fee: _fees[from], chance: chance, slip: slipDie(from));
}

/// What [def] at [refine] adds while worn, as a short line
/// (`+2 damage`, `+1 AC, +12 max HP`); empty for nothing.
String refineBonus(ItemDef def, int refine) {
  if (refine <= 0 || !def.refinable) return '';
  if (def.kind == 2) return '+${(refine + 1) ~/ 2} damage';
  final ac = def.kind == 3 ? refine ~/ 4 : 0;
  return [if (ac > 0) '+$ac AC', '+${3 * refine} max HP'].join(', ');
}

/// The catalogue id and refine level of an instance key
/// (`sword+7[card]` → `sword`, 7).
({String id, int refine}) splitKey(String key) {
  final m = RegExp(r'^([a-z0-9_]+)(?:\+(\d+))?').firstMatch(key);
  if (m == null) return (id: key, refine: 0);
  return (id: m[1]!, refine: int.tryParse(m[2] ?? '') ?? 0);
}

/// The key of [key] at another refine [level] (cards kept).
String withRefine(String key, int level) {
  final m = RegExp(r'^([a-z0-9_]+)(?:\+\d+)?(.*)$').firstMatch(key);
  if (m == null) return key;
  return '${m[1]}${level > 0 ? '+$level' : ''}${m[2]}';
}

/// Names of the equipment slots, as the refine window lists worn items.
const _slotNames = ['Upper headgear', 'Mid headgear', 'Lower headgear', 'Armour', 'Weapon', 'Shield', 'Garment', 'Shoes', 'Accessory', 'Accessory'];

/// One row of the smith's window (`GameView.showRefine`, `RefineRow`): the
/// item [key] on [def], worn in [slot] (null: off its bag stack), with the
/// next attempt worked out against the ore the bag holds ([have], named
/// [oreName]) and the [zeny]; null when a smith cannot work it. Its `id`
/// (`<slot | b>:<key>`) comes back in `refine_strike`.
Map<String, Object?>? refineRow(
  ItemDef def,
  String key,
  int? slot, {
  required int Function(String ore) have,
  required String Function(String ore) oreName,
  required int zeny,
}) {
  if (!def.refinable) return null;
  final level = splitKey(key).refine;
  final step = refineStep(def, level);
  final owned = step == null ? 0 : have(step.ore);
  return {
    'id': '${slot ?? 'b'}:$key',
    'key': key,
    'where': slot == null ? 'Bag' : '${_slotNames[slot]} · worn',
    'level': level,
    'safe': def.refineSafe,
    'bonus': refineBonus(def, level),
    if (step != null)
      'next': {
        'to': step.to,
        'ore': step.ore,
        'oreName': oreName(step.ore),
        'have': owned,
        'fee': step.fee,
        'chance': step.chance,
        'slip': step.slip,
        'gain': refineBonus(def, step.to),
      },
    'blocked': step == null
        ? 'Refined to the limit: +$maxRefine.'
        : owned < 1
        ? 'You need 1 ${oreName(step.ore)}.'
        : zeny < step.fee
        ? 'Not enough zeny: the fee is ${step.fee} z.'
        : '',
  };
}

/// The rules in a few lines (the refine window's footnote).
const refineRules =
    'Phracon refines to +4, Emveretarcon to +7, then Oridecon (weapons) or Elunium (armour) to +10. '
    'Safe up to +4 on armour and +4 to +7 on weapons (the lower the weapon level, the further); past that it can fail, '
    'and a failure loses a roll of levels: d2 up to +5, d4 at +6 and +7, d6 at +8, d8 at +9. Nothing ever breaks.';

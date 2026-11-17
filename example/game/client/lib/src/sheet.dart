/// The Equipment window's combat numbers from the server's live sheet
/// (`PLAYER_SHEET`, server `hub/sheet.rs`): armour class, attack, damage,
/// speed, saves... as the fight rolls them now (buffs, auras, conditions),
/// each marked against its base (gear and passives) with a breakdown
/// tooltip. Pure: the same for the player's own window and an inspected
/// player's.
library;

String _signed(num v) => v >= 0 ? '+$v' : '−${-v}';

/// `+2` / `−1` after a number that moved off its base ('' when it did not).
String _delta(num now, num base) => now == base ? '' : ' (${_signed(now - base)})';

/// The GameView `sheet` (engine `equip.ts` `Sheet`) of a decoded
/// `PLAYER_SHEET` JSON.
Map<String, Object?> sheetView(Map<String, Object?> s) {
  int n(String k) => (s[k] as num?)?.toInt() ?? 0;
  final tips = <String, String>{};
  final marks = <String, String>{};
  // Above the base is `up` (green), below `down` (red); `lowerIsBetter`
  // for intervals.
  void mark(String row, num now, num base, {bool lowerIsBetter = false}) {
    if (now == base) return;
    marks[row] = (now > base) != lowerIsBetter ? 'up' : 'down';
  }

  String tip(String title, List<String> lines) =>
      '<span class="t">$title</span>${lines.where((l) => l.isNotEmpty).join('<br>')}';

  // Armour class: what monsters roll against, term by term.
  final ac = n('ac'), acBase = n('ac_base');
  final parts = s['ac_parts'] is Map ? s['ac_parts']! as Map : const {};
  int part(String k) => (parts[k] as num?)?.toInt() ?? 0;
  String term(String label, String k) => part(k) == 0 ? '' : '$label ${_signed(part(k))}';
  mark('Armor Class', ac, acBase);
  tips['Armor Class'] = tip('Armor Class $ac', [
    'Armour, shield, refines and level ${part('worn')}',
    term('Magic items, cards and passives', 'items'),
    term('Buffs and auras', 'buffs'),
    term('Conditions', 'conditions'),
    term('Cards right now', 'cards'),
    if (ac != acBase) '<span class="dim">Base $acBase</span>',
  ]);

  // Attack bonus: the d20's, with Bless's die and advantage.
  final toHit = n('to_hit'), toHitBase = n('to_hit_base'), die = n('to_hit_die'), adv = n('advantage');
  final critFrom = n('crit_from');
  mark('Attack bonus', toHit, toHitBase);
  if (adv != 0) marks['Attack bonus'] = adv > 0 ? 'up' : 'down';
  final advText = switch (adv) {
    > 0 => ', advantage',
    < 0 => ', disadvantage',
    _ => '',
  };
  tips['Attack bonus'] = tip('Attack bonus ${_signed(toHit)}', [
    'Without buffs ${_signed(toHitBase)}',
    if (toHit != toHitBase) 'Buffs ${_signed(toHit - toHitBase)}',
    if (die > 0) 'Bless: +1d$die on every roll',
    if (adv > 0) 'Advantage: roll twice, keep the higher',
    if (adv < 0) 'Disadvantage (blinded, poisoned): roll twice, keep the lower',
    critFrom > 0 && critFrom < 20 ? 'Critical hit on $critFrom–20' : 'Critical hit on a natural 20',
  ]);

  // Damage per weapon hit, with the buffs' extra dice.
  final damage = s['damage'] as String?, damageBase = s['damage_base'] as String?, extra = s['damage_extra'] as String?;
  final damageText = damage == null ? null : (extra == null ? damage : '$damage +$extra');
  if (damage != damageBase || extra != null) marks['Damage'] = 'up';
  tips['Damage'] = tip('Damage per hit', [
    if (damageBase != null) 'Without buffs $damageBase',
    if (damage != damageBase && damage != null) 'Now $damage',
    if (extra != null) 'Buffs add $extra',
  ]);

  // Swings per second, Double Attack.
  final interval = n('interval_ms'), intervalBase = n('interval_base_ms'), attacks = n('attacks').clamp(1, 99);
  String perSec(int ms) => ms > 0 ? (1000 / ms * attacks).toStringAsFixed(1) : '-';
  mark('Attacks', interval, intervalBase, lowerIsBetter: true);
  final dbl = n('double_pct');
  tips['Attacks'] = tip('Attacks', [
    'A swing every ${(interval / 1000).toStringAsFixed(2)} s, $attacks blow${attacks == 1 ? '' : 's'} each',
    if (interval != intervalBase) 'Without buffs ${perSec(intervalBase)} / s',
    if (dbl > 0) 'Double Attack: $dbl% for one more swing',
  ]);

  // The rest, as rows of their own when they say something.
  final rows = <List<String>>[];
  final speed = n('speed_pct'), speedBase = n('speed_base_pct');
  if (speed != 100 || speedBase != 100) {
    rows.add(['Speed', speed == speedBase ? '$speed%' : '$speed% (${_signed(speed - speedBase)}%)']);
    mark('Speed', speed, speedBase);
    tips['Speed'] = tip('Walking speed $speed%', ['Without buffs and conditions $speedBase%']);
  }
  final resist = n('resist_pct'), reduce = n('reduce');
  if (resist > 0 || reduce != 0) {
    rows.add([
      'Damage taken',
      [if (resist > 0) '−$resist%', if (reduce != 0) '${_signed(-reduce)} per physical hit'].join(', '),
    ]);
    marks['Damage taken'] = 'up';
  }
  final block = n('block_pct'), parry = n('parry_pct');
  if (block > 0) rows.add(['Block', '$block% of melee blows']);
  if (parry > 0) rows.add(['Parry', '$parry% of melee blows']);

  final dc = n('save_dc'), dcBase = n('save_dc_base');
  mark('Spell DC', dc, dcBase);
  final savesFail = s['saves_fail'] == true;
  if (savesFail) {
    marks['Saves'] = 'down';
    tips['Saves'] = tip('Saves', ['Stunned: every saving throw fails']);
  }
  return {
    'ac': '$ac${_delta(ac, acBase)}',
    'toHit': '${_signed(toHit)}${die > 0 ? ' +1d$die' : ''}$advText',
    'damage': ?damageText,
    'damageType': ?s['damage_type'],
    'attacksPerSec': perSec(interval),
    'proficiency': _signed(n('proficiency')),
    'classMod': _signed(n('class_mod')),
    'spellDc': '$dc${_delta(dc, dcBase)}',
    'saves': ?s['saves'],
    'k': ?s['k'],
    'rows': rows,
    'tips': tips,
    'marks': marks,
  };
}

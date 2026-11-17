import 'package:game_client/src/sheet.dart';
import 'package:test/test.dart';

void main() {
  // A `PLAYER_SHEET` of server `hub/sheet.rs` (an acolyte under Angelus,
  // Blessing and Increase Agility, mired).
  const buffed = <String, Object?>{
    'level': 20,
    'ac': 17,
    'ac_base': 16,
    'ac_parts': {'worn': 15, 'items': 1, 'buffs': 3, 'conditions': -2, 'cards': 0},
    'to_hit': 7,
    'to_hit_base': 6,
    'to_hit_die': 4,
    'advantage': 0,
    'crit_from': 20,
    'damage': '1d6+3',
    'damage_base': '1d6+3',
    'damage_extra': null,
    'damage_type': 'neutral',
    'interval_ms': 750,
    'interval_base_ms': 1000,
    'attacks': 1,
    'double_pct': 0,
    'speed_pct': 125,
    'speed_base_pct': 100,
    'resist_pct': 0,
    'reduce': 0,
    'block_pct': 0,
    'parry_pct': 0,
    'saves': {'body': 2, 'reflex': 1, 'mind': 4},
    'saves_fail': false,
    'save_dc': 14,
    'save_dc_base': 14,
    'proficiency': 3,
    'class_mod': 3,
    'k': 2,
  };

  test('live numbers show their change off the base, with the breakdown', () {
    final v = sheetView(buffed);
    expect(v['ac'], '17 (+1)');
    expect(v['toHit'], '+7 +1d4');
    expect(v['attacksPerSec'], '1.3');
    expect(v['spellDc'], '14');
    final marks = v['marks']! as Map;
    expect(marks['Armor Class'], 'up');
    expect(marks['Attacks'], 'up', reason: 'a shorter interval is better');
    expect(marks.containsKey('Spell DC'), isFalse);
    final tip = (v['tips']! as Map)['Armor Class'] as String;
    expect(tip, allOf(contains('Buffs and auras +3'), contains('Conditions −2'), contains('Base 16')));
    expect(tip, isNot(contains('Cards right now')), reason: 'zero terms are left out');
    final rows = v['rows']! as List;
    expect(rows, [
      ['Speed', '125% (+25%)'],
    ]);
  });

  test('a plain sheet reads like the gear sheet', () {
    final plain = {
      ...buffed,
      'ac': 16,
      'to_hit': 6,
      'to_hit_die': 0,
      'interval_ms': 1000,
      'speed_pct': 100,
      'ac_parts': {'worn': 15, 'items': 1},
    };
    final v = sheetView(plain);
    expect((v['ac'], v['toHit'], v['attacksPerSec']), ('16', '+6', '1.0'));
    expect(v['marks'], isEmpty);
    expect(v['rows'], isEmpty);
  });

  test('stunned saves and hurt numbers are marked down', () {
    final v = sheetView({...buffed, 'ac': 12, 'saves_fail': true, 'advantage': -1});
    final marks = v['marks']! as Map;
    expect((marks['Armor Class'], marks['Saves'], marks['Attack bonus']), ('down', 'down', 'down'));
    expect(v['ac'], '12 (−4)');
    expect(v['toHit'], '+7 +1d4, disadvantage');
  });
}

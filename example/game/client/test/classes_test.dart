import 'package:game_client/src/classes.dart';
import 'package:test/test.dart';

void main() {
  test('tier thresholds follow COMBAT.md (30 and 60)', () {
    expect([1, 29, 30, 59, 60, 99].map(tierOfLevel), [1, 1, 2, 2, 3, 3]);
  });

  test('class bodies per tier', () {
    expect([1, 2, 3].map((t) => bodyJob('swordsman', t)), ['swordsman', 'crusader', 'paladin']);
    expect([1, 2, 3].map((t) => bodyJob('mage', t)), ['mage', 'wizard', 'high_wizard']);
    expect([1, 2, 3].map((t) => bodyJob('archer', t)), ['archer', 'hunter', 'sniper']);
    expect([1, 2, 3].map((t) => bodyJob('acolyte', t)), ['acolyte', 'priest', 'high_priest']);
  });

  test('tier defaults to 1 and is clamped; unknown classes are novices', () {
    expect(bodyJob('mage'), 'mage');
    expect(bodyJob('mage', 0), 'mage');
    expect(bodyJob('mage', 9), 'high_wizard');
    expect(bodyJob('novice', 2), 'novice');
    expect(bodyJob('thief'), 'novice');
  });
}

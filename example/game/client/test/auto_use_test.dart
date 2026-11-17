import 'package:game_client/src/auto_use.dart';
import 'package:game_client/src/protocol.dart';
import 'package:test/test.dart';

void main() {
  group('auto-use potions', () {
    test('off until the toggle is on', () {
      expect(usePrefsOf({}), (0, 0));
      expect(usePrefsOf({'autoPotion': false, 'autoPotionHp': 50}), (0, 0));
    });

    test('defaults, chosen thresholds, clamped and "never"', () {
      expect(usePrefsOf({'autoPotion': true}), (autoPotionHpDefault, autoPotionSpDefault));
      expect(usePrefsOf({'autoPotion': true, 'autoPotionHp': 70, 'autoPotionSp': 0}), (70, 0));
      expect(usePrefsOf({'autoPotion': true, 'autoPotionHp': 300, 'autoPotionSp': -5}), (95, 0));
    });

    test('the fields reflect the settings', () {
      final f = autoUseFields({'autoPotion': true, 'autoPotionHp': 30});
      expect([for (final x in f) x['id']], ['autoPotion', 'autoPotionHp', 'autoPotionSp']);
      expect(f[0]['value'], true);
      expect(f[1]['value'], 30);
      expect(f[2]['value'], autoPotionSpDefault);
      expect((f[2]['options']! as List).first, {'value': 0, 'label': 'Never'});
    });

    test('the frame the settings send', () {
      final (hp, sp) = usePrefsOf({'autoPotion': true, 'autoPotionHp': 40, 'autoPotionSp': 25});
      expect(ClientMsg.usePrefs(hp: hp, sp: sp), [0xA7, 40, 25]);
    });
  });
}

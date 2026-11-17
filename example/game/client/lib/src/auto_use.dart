/// Auto-use of HP / SP consumables on missions (Settings → Potions): a
/// toggle and two thresholds, kept with the other settings. The server
/// does the drinking (`USE_PREFS`, `shop.rs` `auto_use`): it knows the
/// cooldown, the running potion and what is carried.
library;

/// Default thresholds (percent) when the toggle is first turned on.
const autoPotionHpDefault = 40;
const autoPotionSpDefault = 20;

/// `(hp, sp)` percent for `USE_PREFS` from the [settings]: `(0, 0)` while
/// the toggle is off; a threshold of 0 leaves that pool alone.
(int, int) usePrefsOf(Map<String, Object?> settings) {
  if (settings['autoPotion'] != true) return (0, 0);
  int pct(String id, int fallback) => ((settings[id] as num?)?.toInt() ?? fallback).clamp(0, 95);
  return (pct('autoPotionHp', autoPotionHpDefault), pct('autoPotionSp', autoPotionSpDefault));
}

/// The settings fields: the toggle and the two thresholds.
List<Map<String, Object?>> autoUseFields(Map<String, Object?> settings) => [
  {
    'id': 'autoPotion',
    'label': 'Auto-use potions',
    'type': 'toggle',
    'value': settings['autoPotion'] == true,
    'hint':
        'On missions: drinks the best healing potion or food when HP falls below the threshold, '
        'and the best SP potion below the SP one (one drink a second, never over a running one).',
  },
  {
    'id': 'autoPotionHp',
    'label': 'Auto-use: HP below',
    'type': 'select',
    'value': (settings['autoPotionHp'] as num?)?.toInt() ?? autoPotionHpDefault,
    'hint': 'Drink a healing potion or food under this much HP (with Auto-use potions on).',
    'options': [
      for (var p = 10; p <= 90; p += 10) {'value': p, 'label': '$p%'},
    ],
  },
  {
    'id': 'autoPotionSp',
    'label': 'Auto-use: SP below',
    'type': 'select',
    'value': (settings['autoPotionSp'] as num?)?.toInt() ?? autoPotionSpDefault,
    'hint': 'Drink an SP potion under this much SP (with Auto-use potions on). Never: HP only.',
    'options': [
      {'value': 0, 'label': 'Never'},
      for (var p = 10; p <= 90; p += 10) {'value': p, 'label': '$p%'},
    ],
  },
];

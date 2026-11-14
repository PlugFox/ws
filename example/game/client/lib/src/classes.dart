/// Class lines, tiers and their character bodies (example/docs/COMBAT.md
/// §1.2): a class is chosen at creation, its title and body change with
/// the tier, and the tier follows the character's level.
library;

/// Skill-tree tier of a character level: 1 below 30, 2 below 60, else 3
/// (the server's `combat::tier`).
int tierOfLevel(int level) => level < 30 ? 1 : (level < 60 ? 2 : 3);

/// Body of each class line per tier (index 0 = tier 1). The names are the
/// engine's body slugs (`GameView.bodySpriteIndex`).
const classBodies = <String, List<String>>{
  'swordsman': ['swordsman', 'crusader', 'paladin'],
  'mage': ['mage', 'wizard', 'high_wizard'],
  'archer': ['archer', 'hunter', 'sniper'],
  'acolyte': ['acolyte', 'priest', 'high_priest'],
};

/// Body slug of a class (`archetype`, the character's class id) at a tier
/// (1..3, clamped). Unknown classes (and the old `novice`) get the novice
/// body. Other players are tier 1 until the wire carries their tier.
String bodyJob(String archetype, [int tier = 1]) {
  final line = classBodies[archetype];
  if (line == null) return 'novice';
  return line[(tier < 1 ? 1 : (tier > 3 ? 3 : tier)) - 1];
}

/// Bodies of the class lines not playable yet (shown locked at creation),
/// by the all-bodies pack's id (`client/bodies/sprites/<id>_<m|f>.json`).
const lockedBodies = <String, String>{
  'thief': 'thief',
  'merchant': 'merchant',
  'super_novice': 'super_novice',
  'ninja': 'ninja',
  'gunslinger': 'gunslinger',
  'taekwon': 'taekwon_boy',
};

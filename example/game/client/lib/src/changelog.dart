/// Update notes shown at the town's notice board (newest first). Add an
/// entry here with every playable change; the board reads nothing else.
library;

typedef ChangelogEntry = ({String title, String date, List<String> items});

const changelog = <ChangelogEntry>[
  (
    title: 'Townsfolk, emotes and flags',
    date: '2026-11-13',
    items: [
      'Izlude is alive: townsfolk of every class stroll the streets, sit down to rest and chat with emotes.',
      'Emotes: Alt+1…0 (pick which emote each key plays in the settings, F10).',
      'Flags: Ctrl+1…0 plants a flag where the pointer is, for everyone on the map.',
      'C sits down and stands up in town (rebindable).',
      'The Kafra employee now stands in front of the market tent and offers every service: missions, parties, skill reset and restyle.',
      'The hotbar is one row of ten big slots on 1…0; every key can be rebound in the settings.',
      'The town square is greener: painted ground, denser grass with flowers and mushrooms.',
    ],
  ),
  (
    title: 'Monster skills and bosses',
    date: '2026-11-13',
    items: [
      'Monsters cast their own skills; Baphomet waits at the end of the mission.',
      'Victory and defeat screens; the party returns to town together.',
      'Gamepad and touch controls.',
    ],
  ),
  (
    title: 'Zones and conditions',
    date: '2026-11-13',
    items: [
      'Lasting ground skills: fire walls, swamps, wards, traps and barriers.',
      'Burning, poison, freezing, stun, slow and more, on monsters and players alike.',
    ],
  ),
  (
    title: 'Skills and the hotbar',
    date: '2026-11-13',
    items: [
      'Skill trees per class (K), skill points per level, a hotbar with cooldowns and cast bars.',
      'Aldric the Skill Master resets learned skills.',
    ],
  ),
  (
    title: 'Combat',
    date: '2026-11-13',
    items: [
      'Dice combat, weapons, experience and levels, shared within the party.',
      'Classes change their look at levels 30 and 60.',
    ],
  ),
  (
    title: 'Characters and missions',
    date: '2026-11-13',
    items: [
      'Character slots, the stylist (hair and dyes).',
      'Mission parties: public or private with a code, up to four players.',
      'Waves of monsters within the light radius.',
    ],
  ),
];

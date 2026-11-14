/// Update notes shown at the town's notice board (newest first). Add an
/// entry here with every playable change; the board reads nothing else.
library;

typedef ChangelogEntry = ({String title, String date, List<String> items});

const changelog = <ChangelogEntry>[
  (
    title: 'Interface polish',
    date: '2026-11-14',
    items: [
      'A quieter chat: a grey see-through box with smaller text and no title bar. Move it by its input bar, resize it from its top right corner (down to three lines); both are remembered. Clicks on the log go through to the game: scroll it with the wheel, or hold Shift (or type) to select lines. When nobody talks it fades away, all but the input, and comes back with a new line or the mouse over it.',
      'Every entry of the game menu and the player menu says what it does, so the tiles line up.',
      'A wider Settings window, one line per setting. What a setting does is behind its (?): hover it, tap it on touch, or move onto the setting with the keyboard or a controller.',
      'Start a mission at once: the mission window has a Start button. Alone, it sends you straight in with the difficulty and curses you picked; a party leader starts the party once everyone is ready. A party member sees why they cannot.',
      'Messages at the top of the screen wrap to fit and line up under the mission timeline and the boss bars instead of hiding behind them, on a phone too. Back in town the greeting is short; the keys are listed on your first arrival only.',
      'A cast cut short by moving (or a stun) no longer shows its bolts flying: what the server called off disappears on screen too, for monster skills as well.',
    ],
  ),
  (
    title: 'Mobbed!',
    date: '2026-11-14',
    items: [
      'The game has a name, a logo and a home: mob.plugfox.dev. Add it to your home screen to play it like an app.',
      'Loading screens between places, with art and tips (some drawn from the mission itself), and a new title screen that fits any display.',
      'A new character screen: what each class is good at on a chart, its role, hit die, weapons and signature skills played on your hero. Super Novice, Ninja, Gunslinger and Taekwon are on the way.',
    ],
  ),
  (
    title: 'After the battle',
    date: '2026-11-14',
    items: [
      'A mission now ends with its results: damage per second over time for the whole party, kills, assists, falls and revives, damage by skill, and everything you earned and picked up.',
      'The window stays open back in town until you close it.',
    ],
  ),
  (
    title: 'Risk and reward',
    date: '2026-11-14',
    items: [
      'The party leader can raise a mission from Normal up to Hell and add curses: one blow downs you, slower feet, no potions, darkness, an early fog, frenzied monsters, a swarm.',
      'The riskier the run, the bigger the bonus for clearing it: extra zeny, experience and chest rolls. More monsters drop a little more, but each is worth less.',
    ],
  ),
  (
    title: 'The fields come alive',
    date: '2026-11-14',
    items: [
      'Herbs and mushrooms grow on the grass of mission fields. Each blow takes one leaf; they drop herbs of their colour and spores. Auto-attack leaves them alone, click one to cut it.',
      'Birds, brooks, surf, wind in the gusts and the bustle of the Izlude square; footsteps change with the ground. Ambience has its own volume (quiet by default) and footsteps can be turned off in Settings.',
      'Big fights sound clearer: a crowd of identical hits plays as one, and your own blows always come through.',
    ],
  ),
  (
    title: 'Email and Telegram on your account',
    date: '2026-11-14',
    items: [
      'When you create an account you can add an email address and a Telegram username. Both are optional and never shown to other players.',
      'They will be used later to recover a lost password and for notifications (a party invite, a friend coming online, a restart). Add, change or remove them any time in Settings, Account.',
    ],
  ),
  (
    title: 'A dropped connection is not the end',
    date: '2026-11-14',
    items: [
      'If your connection drops, the game reconnects on its own and says so on screen; your controls pause until you are back.',
      'On a mission your character holds its ground for a minute (it still takes hits, so no hiding behind a cable) and you return to the same fight with the same health. Your party sees "disconnected" over your name meanwhile.',
      'In town you come back to the same spot and channel. Reloaded the page mid-mission? After signing in you can return to the battle or leave it.',
      'An expired sign-in renews itself; if it cannot, you land on the sign-in screen with the reason.',
    ],
  ),
  (
    title: 'Boss fights worth remembering',
    date: '2026-11-14',
    items: [
      'Eclipse, Vocal, Mastering and the Angeling now fight with phases, yells and signature moves: charges and leaps, sound waves on the beat, slime pools and porings that the big one swallows to heal, holy crosses, sweeping beams and a shielded Holy Nova.',
      'Red zones show where each blow lands and fill up until it does; the next one in a sequence lights up as the one before lands. Stand in the quiet gaps, close in under a halo, hide in the blue sanctuaries.',
      'A bar at the top shows the boss or elite with its phases, shield and the cast it is preparing. Burst it down to break a channel ("Interrupt!") or a shield before it finishes; kill the adds before they are eaten.',
    ],
  ),
  (
    title: 'Hall of Fame',
    date: '2026-11-14',
    items: [
      'Rankings: the top adventurers by level, monsters slain, MVPs (bosses beaten with your party) and zeny, with a podium for the best three and your own rank wherever you are. Open the Ranking tab of the Social window with Alt+L, from the menu, or ask this notice board.',
      'Your record counts from now on: kills, MVPs, missions won, falls and your best combo.',
      'The sign-in screen shows how many players are online and your ping; in town the count sits beside the minimap.',
    ],
  ),
  (
    title: 'A roomier bag and one Social window',
    date: '2026-11-14',
    items: [
      'The inventory is larger, with bigger slots. Identical items share one slot with a count; a refined or carded copy keeps a slot of its own.',
      'Party and Friends are now tabs of one Social window (the speech-bubble button in Basic Info). Alt+Z opens the Party tab, Alt+H the Friends tab.',
      'The browser no longer zooms, selects text or opens its own menu over the game; text fields still copy and paste.',
    ],
  ),
  (
    title: 'Tell us what you think',
    date: '2026-11-14',
    items: [
      'Liesel the Scribe now stands beside this notice board: report a bug, suggest an idea or tell us what feels too hard or too easy. Kafra employees take feedback too.',
      'Pick the type and how much it matters, give it a title and describe it in detail: the developers read every report.',
      'The game and server versions now show in the corner of the title screen and at the bottom of the settings.',
    ],
  ),
  (
    title: 'Style meter',
    date: '2026-11-14',
    items: [
      'Kill combo: every kill you land adds to a counter under the minimap; keep killing before the bar runs dry or it drops (being downed ends it too).',
      'Style ranks D, C, B, A, then S, SS and SSS: each needs more kills and leaves less time between them. An announcer calls every new rank (an AI-generated voice).',
    ],
  ),
  (
    title: 'Merchants and gear',
    date: '2026-11-14',
    items: [
      'Izlude has merchants: the Tool Dealer (potions), Dega the Weapon Dealer and the Armor Dealer. Buy, or sell anything back at half price.',
      'Zeny drops from every kill (more from tougher monsters) and a bonus for clearing a mission.',
      'Equipment: armour, shields, headgear, garments, shoes and accessories in the ten slots; armour class follows what you wear (light, medium, heavy).',
      'New heroes start with their class weapon, a Cotton Shirt (and a Guard for swordsmen), five Red Potions and 100 zeny.',
      'Potions heal over a few seconds and never stack: R drinks a healing potion, T an SP potion, or click the HP / SP orb.',
      'Inventory (Alt+E) and Equipment (Alt+Q) show your items with their numbers: double-click to drink or wear.',
      'Monsters drop loot: jellopies, fluff, shells, herbs and apples float up from the fallen, with the odd weapon; elites and the Angeling drop for every party member. Sell the spoils to any merchant, eat the herbs on the spot (Green Herb cures poison).',
    ],
  ),
  (
    title: 'Steel and sound',
    date: '2026-11-14',
    items: [
      'Heroes hold their weapons: the bow, sword or rod shows in the combat stance and in every swing.',
      'Swings, hits and monsters have their sounds again: porings squeak, zombies groan, arrows twang.',
      'Monsters flinch when hit instead of walking through blows (bosses still endure).',
      'Level up mid-fight: HP and SP refill, the angel and the chime, and nearby monsters are thrown back.',
      'Monster spells wind up longer and cover less ground: you can step out of the red.',
      'New characters arrive on the square by the Kafra; back from a mission you land a little south of it.',
      'Space on a mission casts your travel skill (Back Slide, Blink, Charge…): no hotbar slot needed.',
      'Fixed: porings no longer leave a frozen copy behind when they burst; character previews stay in their frames.',
    ],
  ),
  (
    title: 'Interface, second pass',
    date: '2026-11-13',
    items: [
      'Sharp at any scale: windows, buttons, bars and sliders are drawn crisply; icons stay clean.',
      'Basic Info: HP, SP and EXP bars, icon buttons; folds into a two-line strip.',
      'Round minimap with the map name, your coordinates, zoom and fold on it; party members and the boss are marked.',
      'Glass life and mana orbs on missions; the hotbar shows on missions, and in town only while the skill tree is open.',
      'Skill tree: hover or hold a skill to read it, click it to see what it needs, drag its icon onto the hotbar.',
      'Back Slide is now an Archer skill: travel skills belong to the class trees.',
      'Esc menu (character select, log out, settings, controls, full screen); F1 debug overlay with graphs.',
      'Sound settings (music at 25% by default), Space also talks to NPCs, Alt+E inventory (shops coming).',
    ],
  ),
  (
    title: 'A new interface',
    date: '2026-11-13',
    items: [
      'Every window in the classic white-and-blue style: sign-in, characters, dialogs, options, skills.',
      'Life and mana orbs in the bottom corners; Basic Info with level, HP, SP and experience.',
      'Windows can be dragged and folded, and remember where you left them; Options → Interface: scale and reset.',
      'Equipment window (Alt+Q); Basic Info folds with Alt+V.',
      'The minimap folds, zooms (− / + or the wheel) and shows your coordinates.',
      'Phones get a compact layout down to 360 px wide.',
    ],
  ),
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

/// Update notes shown at the town's notice board (newest first). Add an
/// entry here with every playable change; the board reads nothing else.
library;

typedef ChangelogEntry = ({String title, String date, List<String> items});

const changelog = <ChangelogEntry>[
  (
    title: 'Three new expeditions',
    date: '2026-11-14',
    items: [
      'The Kafra\'s "Choose an expedition" opens a new window: every mission with its picture, level band and whether it suits your level, and a full page on the one you pick with its boss, dangers, best loot and difficulty. Under-level? It warns you and asks once more. On phones the list and the page are two screens.',
      'Prontera Culvert (levels 15–30): thief bugs, rats and bats under the capital. Break the egg clusters before they hatch, mind the sewage banks and the flush, and face the Thief Bug Queen, the Rat King and the Golden Thief Bug, whose golden shell turns spells back until you crack it. Fire fizzles in sewage; cold freezes it into safe ice.',
      'Sograt Desert (levels 30–45): sandstorms every 2½ minutes (get behind cover; spells drift with the wind), antlion pits, Hodes hunting from under the sand, Muka cacti that prick back. Survive Sirocco and his pack, the Sand Colossus and Phreeoni rising from the great sinkhole.',
      'Payon Caves (levels 45–60): a dark shrine full of undead and fox spirits. Strike the braziers to kindle them and ring the shrine bells to stun the undead; face the Weeping Bride, the Jiangshi General and Moonlight Flower. New condition: Feared.',
      'Monsters can spawn with ★ to ★★★ affixes (Hasted, Armoured, Vampiric, Fire-enchanted, Teleporting, Summoner, Thorns and more), each with its own aura and telegraphed tricks. Starred monsters pay more experience, zeny and loot; their minions pay nothing.',
    ],
  ),
  (
    title: 'Clearer skills and controls',
    date: '2026-11-14',
    items: [
      'Skill indicators show each spell\'s real shape: lines, walls, cones, a reticle on single targets, bounce reach. A quiet look that can\'t be mistaken for a monster\'s attack.',
      'Every skill level now improves something: no more dead levels. Mystical Amplification now empowers your next damaging spell.',
      'Click to move: left-click the ground to walk there around obstacles, or hold to steer; click a monster, a bubble or a portal to walk to it. Touch has a "Tap to move" option.',
      'Elites and bosses show a health bar under their feet; players show a mana bar under their health.',
      'Steadier movement: inputs that arrive together are no longer lost, so walks end where you aimed them.',
    ],
  ),
  (
    title: 'The Thief and the Peco Peco',
    date: '2026-11-14',
    items: [
      'New class: the Thief, who becomes an Assassin at 30 and an Assassin Cross at 60. Poisons, Sonic Blow, Hiding and Cloaking to slip through the horde: strike out of hiding to attack with advantage. Katars arrive at the weapon merchant.',
      'Crusaders and Paladins can ride a Grand Peco: learn Peco Peco Ride and press its hotbar key, in town or on a mission. New cavalry skills: Cavalier Mastery, Spear Mastery, Brandish Spear, Spear Quicken and, for Paladins, Spiral Pierce.',
      'Archers learn Piercing Arrow: one shot down a line that hits every monster it passes.',
    ],
  ),
  (
    title: 'Parties that stay together',
    date: '2026-11-14',
    items: [
      'Your party stays together after a mission, when you switch characters and when you log out. The party window shows who is in town, on a mission or offline.',
      'No more Ready button: the party leader starts a mission whenever they like, and whoever is in town comes along. Parties now hold up to eight players.',
      'Sessions renew themselves, so no more "token expired" at the character select, which now also has a Log out button.',
    ],
  ),
  (
    title: 'Sharper fights',
    date: '2026-11-14',
    items: [
      'Melee hits land at arm\'s reach, and monsters only strike when they touch you. Monsters attack less often, and bosses pause between their big moves.',
      'Party buffs (Blessing, Increase AGI, …) go to the nearest party member who lacks them, you included; auto-cast waits until someone needs them. Heal picks whoever is hurt most. Support buffs at their top level outlast their cooldown.',
      'New setting "Manual aiming": skills aimed at a monster pick the one nearest your mouse, and ground skills land where you point.',
      'Travel skills (Back Slide, Blink, Space) go where you are walking, or toward the mouse when you stand still.',
      'Fire Wall grows from 3 to 10 flames with its level.',
      'Potions heal twice as much and Blue Potions restore far more SP. A Food Vendor in Izlude sells cheap fruit, meat and grapes (for SP).',
    ],
  ),
  (
    title: 'Looks and handling',
    date: '2026-11-14',
    items: [
      'Hats, glasses and masks show on your character, other players and the character select screen.',
      'Characters face what they attack or cast at; casting has its own pose, and spears and other weapons no longer vanish mid-swing.',
      'Eclipse really leaps between its landing circles; Heal is a spinning green column; Fire and Cold Bolts fall from the sky.',
      'The game has its own mouse cursor: Classic or Gilded, in three sizes (Settings, Graphics), with a sword over monsters, a speech bubble over NPCs and a grabbing hand over loot.',
      'Character creation: every class stands the same height, the pose buttons match, and skill effects no longer play on your hero.',
      'Shops have a cart: buy and sell several items at once, see the total and what you will have left. Quantities by +/−, +10, Max or typing; Shift+click takes a whole stack; items drag between the Inventory, the shop and the trade window. The trade window shows who has pressed OK and Trade and warns when an offer changed.',
    ],
  ),
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

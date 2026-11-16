/// The Adventurer's Guide (`GameView.showGuide`, engine `guide.ts`): the
/// game's places and mechanics, one topic per page. Keys are the player's
/// own bindings and the expeditions come from the live mission ladder, so
/// the text follows what the game does; the rest is written here: keep it
/// in step with the rules when they change (changelog.dart too).
///
/// Blocks: `{p}`, `{list}`, `{keys: [[key, what]]}`, `{tip}`, `{figure:
/// {diagram?, icons?, labels?, caption?}}`, `{glossary: true}` (the rules
/// glossary, drawn by the engine from glossary.json). Text takes `**bold**`
/// and glossary links `[[id]]` / `[[id|text]]` (assets/glossary.toml ids);
/// rules words the glossary knows link on their own.
library;

import 'key_bindings.dart';
import 'protocol.dart';

String _k(KeyBinding? b, String fallback) {
  final c = b?.caption ?? fallback;
  return c == '␣' ? 'Space' : c;
}

Map<String, Object?> _topic(String id, String title, String summary, String icon, List<Map<String, Object?>> blocks) => {
  'id': id,
  'title': title,
  'summary': summary,
  'icon': icon,
  'blocks': blocks,
};

Map<String, Object?> _p(String text) => {'p': text};
Map<String, Object?> _list(List<String> items) => {'list': items};
Map<String, Object?> _tip(String text) => {'tip': text};
Map<String, Object?> _keys(List<List<String>> rows) => {'keys': rows};
Map<String, Object?> _fig({
  String? diagram,
  List<String>? icons,
  List<String>? labels,
  List<String>? pictures,
  String? caption,
}) => {
  'figure': {'diagram': ?diagram, 'icons': ?icons, 'labels': ?labels, 'pictures': ?pictures, 'caption': ?caption},
};

/// The guide's topics for [binds], the mission [ladder] (`MISSION_LIST`,
/// may be empty before a Kafra was asked) and [assets] (the asset base URL).
List<Map<String, Object?>> guideTopics(Bindings binds, List<MissionCard> ladder, String assets) {
  final slots = binds.slotCaptions.where((c) => c.isNotEmpty);
  final slotKeys = slots.isEmpty ? '1–0' : '${slots.first}–${slots.last}';
  final talk = _k(binds.interact, 'F');
  // The live ladder once a Kafra sent it; before that the four of launch.
  final live = ladder.where((c) => !c.soon).toList();
  final open = <({String name, int min, int max, String boss, String art})>[
    if (live.isNotEmpty)
      for (final c in live)
        (name: c.name, min: c.band.min, max: c.band.max, boss: c.bosses.isEmpty ? '' : c.bosses.last, art: c.art.isEmpty ? c.map : c.art)
    else ...[
      (name: 'Poring Island Survival', min: 1, max: 15, boss: 'Angeling', art: 'prt_fild08'),
      (name: 'Prontera Culvert', min: 15, max: 30, boss: 'Golden Thief Bug', art: 'prt_sewb1'),
      (name: 'Sograt Desert', min: 30, max: 45, boss: 'Phreeoni', art: 'moc_fild17'),
      (name: 'Payon Caves', min: 45, max: 60, boss: 'Moonlight Flower', art: 'pay_dun04'),
    ],
  ];
  return [
    _topic('start', 'Getting started', 'What Mobbed! is and your first steps', 'monster:poring', [
      _fig(icons: ['monster:poring', 'monster:poporing', 'monster:angeling'], caption: 'Your first foes'),
      _p(
        'Mobbed! is a survival game: you go out on **expeditions** where the horde comes at you in waves, '
        'getting bigger and stronger by the minute. Hold out, beat the boss and come back richer.',
      ),
      _list([
        'Walk with **WASD** (or click the ground). Talk to people with **$talk**.',
        'The **Kafra** in the town centre sends you on expeditions.',
        'Stand still to attack on your own; your skills are on the hotbar (**$slotKeys**).',
        'Each level brings a **skill point**: spend it in the skill tree (**K**).',
        'Better gear and potions come from the shops in town and from monster drops.',
      ]),
      _tip('Start with Poring Island: it is made for level 1, alone or with friends.'),
    ]),
    _topic('controls', 'Controls', 'Keys, mouse, gamepad and touch', 'skill:tf_backsliding', [
      _keys([
        ['W A S D', 'walk (the arrow keys too); **Q** / **E** turn the camera'],
        ['Left click', 'walk there around obstacles, hold to steer; a monster: walk into reach'],
        [talk, 'talk to the nearest person (also Space in town)'],
        [slotKeys, 'hotbar skills (a chained slot casts its next skill per press; hold to rotate)'],
        [_k(binds.ai, '`'), 'the **AI** on / off: auto-cast of the skills marked **A** in the tree'],
        [_k(binds.travel, 'Space'), 'on expeditions: your travel skill (Back Slide, Blink, …)'],
        ['${_k(binds.potion, 'R')} / ${_k(binds.potion2, 'T')}', 'drink a healing / SP potion'],
        ['K', 'skill tree'],
        ['${_k(binds.items, 'Alt+E')} / ${_k(binds.equip, 'Alt+Q')}', 'inventory / equipment'],
        ['${_k(binds.party, 'Alt+Z')} / ${_k(binds.friends, 'Alt+H')}', 'party / friends'],
        [_k(binds.sit, 'C'), 'sit down in town'],
        ['Enter', 'chat'],
        ['Alt+1–0 / Ctrl+1–0', 'emotes / ping flags the party sees'],
        ['Esc / F10', 'menu, close a window / settings and key bindings'],
      ]),
      _p(
        '**Gamepad**: left stick walks, right stick aims; the face buttons and RB are slots 1–5, hold LB for 6–10. '
        '**Touch**: a joystick on the left and skill buttons on the right; tap a button to cast, drag it to aim.',
      ),
      _tip('Every key can be changed in Settings (F10) → Keys.'),
    ]),
    _topic('town', 'Izlude', 'Who is who in town', 'item:emperium', [
      _list([
        '**Kafra Employee**: expeditions, parties by code or from the list of public ones, hair and dyes, forgetting skills, feedback and this guide.',
        '**Dega the Weapon Dealer** and the **Armor Dealer** keep shop in the west hall, the **Tool Dealer** (potions) in the east house; the **Food Vendor** sells cheap fruit and meat by the south-west market stalls.',
        '**Mirabel the Hatter**, in the north-west market: hats stitched from monster drops and a few coins.',
        '**Kylick the Stylist**: a new hair style and dyes.',
        '**Aldric the Skill Master**: forget every skill and spend the points again.',
        "**Notice Board**: what's new, and the Hall of Fame.",
        '**Liesel the Scribe**: report a bug or suggest an idea.',
      ]),
      _p('Town is safe: no monsters. Players **trade** with each other in town only (right click a player).'),
    ]),
    _topic('classes', 'Classes', 'Six class lines and how they grow', 'class:swordsman', [
      _fig(
        icons: ['class:swordsman', 'class:mage', 'class:archer', 'class:acolyte', 'class:thief', 'class:merchant'],
        labels: ['Sword', 'Mage', 'Archer', 'Acolyte', 'Thief', 'Merchant'],
      ),
      _list([
        '**Swordsman** → Crusader → Paladin: tough, up close, taunts the horde. Rides a Peco Peco.',
        '**Mage** → Wizard → High Wizard: fire, ice and lightning over big areas.',
        '**Archer** → Hunter → Sniper: at range, piercing arrows and traps.',
        '**Acolyte** → Priest → High Priest: heals and blesses the party.',
        '**Thief** → Assassin → Assassin Cross: poison, quick blows and hiding.',
        '**Merchant** → Alchemist → Creator: an axe, war cries for the party and fire bottles.',
      ]),
      _p(
        'The next title comes on its own at **level 30** and **level 60**, with new skills to learn. '
        'You get a skill point every level (and one more at 30 and 60); the Skill Master gives them back if you want another build.',
      ),
    ]),
    _topic('combat', 'How combat rolls work', 'Attack rolls, AC, saves, k and the element wheel', 'cc:axis_damage', [
      _p(
        'Every fight is decided by a [[d20]]: a twenty-sided die plus a bonus, against a number; each point of bonus is 5%. '
        'Hover or tap an underlined word anywhere in the game to read what it means, click it to pin it.',
      ),
      _p(
        '**Weapon blows** (your [[basic_attack|basic attacks]] and weapon skills such as Bash) roll an [[attack_roll|attack roll]]: '
        'd20 + your Attack bonus against the monster\'s [[ac|Armor Class]]. Reach it and you hit; a 1 always misses, '
        'a 20 always hits and may become a [[crit|critical hit]], its dice rolled twice.',
      ),
      _list([
        'Attack bonus = [[proficiency|Proficiency]] (+2 at level 1, +1 every 12 levels) + [[class_mod|class modifier]] (+3, +4 at 30, +5 at 60) '
            '+ [[weapon_level|weapon level]] − 1 + "to hit" from your gear.',
        'Damage = the [[weapon_die|weapon die]] + class modifier + weapon level − 1 + refine + "damage on every weapon hit".',
        'Monsters roll against **your** AC the same way: every +1 AC is 5% fewer hits.',
      ]),
      _p(
        '**Spells** (bolts, novas, storms: anything with its own dice) never miss. The monster makes a [[save|saving throw]] instead: '
        'd20 + its save bonus against your [[dc|spell DC]], 8 + Proficiency + class modifier. A success halves the damage ("save for half") '
        'or shrugs off the condition ("save or Stunned"). Your weapon and its bonuses do not count for spells.',
      ),
      _list([
        'Three saves: [[save_body|Body]] (poison, stun, fire), [[save_reflex|Reflex]] (blasts, ice, snares) and [[save_mind|Mind]] (fear, taunts, blindness).',
        'You roll them too: your class save gets your full Proficiency, the other two half of it. The Equipment window lists yours.',
        'Bosses turn a failed save into a success three times a fight ([[legendary_resistance|legendary resistance]]).',
      ]),
      _p(
        '**(k)**: skill dice marked [[k|(k)]] are rolled k times, and k grows by one every 15 character levels: '
        "Fire Bolt's 1d4 is 3d4 at level 30. Weapon blows do not use k; martial classes strike more often instead "
        '([[extra_attack|Extra Attack]] at 30 and 60).',
      ),
      _fig(diagram: 'wheel', caption: 'Each arrow: double damage'),
      _p(
        '**Damage types**: every hit is [[physical|physical]], [[fire|fire]], [[cold|cold]], [[lightning|lightning]], [[earth|earth]], '
        '[[poison|poison]], [[radiant|radiant]], [[necrotic|necrotic]] or [[force|force]]. A monster\'s [[element|element]] takes double, half '
        'or none of some, after the dice: fire burns Earth and Undead, cold Fire, lightning Water, earth Wind, '
        'radiant the Undead, Shadow, Poison and Ghost.',
      ),
      _tip('The Mage skill Sense shows a monster\'s AC, saves and resistances when you hover it.'),
    ]),
    _topic('expeditions', 'Expeditions', 'Where to fight and how a run goes', 'item:clover', [
      _fig(diagram: 'timeline', caption: 'The run clock: waves, elites, the boss'),
      _p("Talk to the **Kafra** and pick **Choose an expedition**: each has a level band, its bosses and what it drops."),
      _list([
        for (final c in open) '**${c.name}**: levels ${c.min}–${c.max}${c.boss.isEmpty ? '' : ', boss ${c.boss}'}.',
      ]),
      _list([
        'Monsters come in **waves**, more each time, and gain levels as the clock runs.',
        '**Elites** (mini-bosses) arrive at set times, the **boss** at the end. Beat the boss or hold out to the time limit to win.',
        'Every kill gives experience and zeny; a win adds a **clear bonus**. Drops float up from the fallen: walk over them.',
        'When nobody on the expedition is standing, the run is lost.',
      ]),
      _fig(pictures: [for (final c in open.take(4)) '$assets/client/ui/missions/${c.art}-320.webp']),
    ]),
    _topic('horde', 'The horde', 'Light, waves and kinds of monsters', 'monster:fabre', [
      _fig(diagram: 'light', caption: 'Outside your light monsters are only shadows'),
      _p('Your **light** shows what is near; whatever is out in the dark stays hidden until it comes close. Some places and curses make it smaller.'),
      _list([
        '**Swarms** come by the dozen and fall fast.',
        '**Chasers** run you down; **tanks** are slow but take a beating.',
        '**Elites** are mini-bosses with their own attacks and a health bar under them.',
        '**Bosses** end the run: a bar on top shows their health and what they are preparing.',
        '**Plants** (herbs, mushrooms) do not fight: click to cut them for drops.',
      ]),
      _fig(diagram: 'telegraph', caption: 'A red area fills up, then the blow lands'),
      _tip('Red areas on the ground are attacks on their way: walk out before they fill. Hard hits can **interrupt** a boss preparing a big move.'),
    ]),
    _topic('affixes', 'Starred monsters', 'Affixes: more danger, more loot', 'bonus:berserk', [
      _fig(diagram: 'stars', caption: '★ Magic · ★★ Rare · ★★★ Exalted'),
      _p(
        'Some monsters arrive with **affixes**: Hasted, Armoured, Vampiric, Fire- or Cold-enchanted, Teleporting, Summoner, Thorns and more. '
        'Each shows its own aura and telegraphs its tricks.',
      ),
      _list([
        'Every star adds health and pays **more experience and zeny** and better drop chances.',
        'Minions they summon and their mirror copies pay nothing.',
      ]),
    ]),
    _topic('skills', 'Skills and auto-cast', 'The hotbar, aiming and auto-cast', 'skill:mg_firebolt', [
      _fig(diagram: 'hotbar', caption: 'A green frame: the AI casts that skill by itself'),
      _p(
        'Learn skills in the tree (**K**) and drag them onto the hotbar. Skills cost **SP** and have a cooldown; '
        'casting holds you in place, and walking away cancels it (the SP comes back).',
      ),
      _list([
        '**Auto-cast**: click the **A** badge of a skill in the tree (or right click a slot). No hotbar slot needed: '
            'while the **AI** is on (the button left of the hotbar, or ${_k(binds.ai, '`')}) it casts the skill whenever it is ready and finds its own target: '
            'the nearest monster or the best line, the party member missing a buff, the most hurt for Heal. '
            'Heals go first, then buffs, then attacks. With nobody to help it waits and spends nothing; whatever you press goes first.',
        '**Chains**: drop up to five skills on one slot (onto its **+** while the tree is open). Each press casts the next one that is ready, '
            'skipping what is cooling, short of SP or without a target; **hold** the key and the rotation goes on. After 3 s it starts over.',
        'Skills you press yourself aim at the **mouse**; with **Manual aiming** (Settings) they pick the monster nearest the pointer and lay ground spells right on it.',
        'Travel skills go where you walk, or toward the mouse when you stand still.',
        'Standing still you **attack on your own**; walking stops it.',
      ]),
      _fig(diagram: 'aim', caption: 'Manual aiming: the monster nearest the pointer'),
    ]),
    _topic('companions', 'Homunculus and cart', "The Merchant line's companions", 'skill:am_callhomun', [
      _p(
        'An **Alchemist** calls a **homunculus**: a pet that fights beside you, levels with you and keeps its level between runs. '
        'The first **Call Homunculus** asks which one, and the choice is for good (changing it later costs 5,000 zeny in town and resets its Bond).',
      ),
      _list([
        '**Lif** heals you, **Amistr** holds the horde off you, **Filir** finishes the wounded, **Vanilmirth** bolts the crowd from afar.',
        'Its plate sits under Basic Info: HP, SP, experience, **Bond** and its skills. **Aggressive**, **Defensive** or **Passive** set how far it looks for a fight; **Rest** lets it heal.',
        'Press the Call Homunculus slot again while it is out: aimed at a monster it goes for that one, otherwise it comes back to you.',
        'Bond grows with every mission cleared together. At **level 60**, Bond 75 and with a **Stone of Sage** (Pharmacy) it evolves in town.',
        'Plants (**Summon Flora**) and **Sphere Mines** take root where you cast them and fight for a while; monsters may bite them instead of you.',
        'Allies stand on a **teal ring**; your own is the brighter one. Your attacks never hurt them.',
        "**Pushcart** rolls behind a Merchant who learned it: more room in the bag, and the cart skills' power grows with what it carries.",
        '**Discount** and **Overcharge** move shop prices; the shop shows the old price struck out. **Pharmacy** brews potions and bottles from herbs and drops.',
      ]),
    ]),
    _topic('survival', 'Staying alive', 'Potions, being downed and the fog', 'item:red_potion', [
      _fig(
        icons: ['item:red_potion', 'item:orange_potion', 'item:blue_potion', 'item:green_potion'],
        labels: ['HP', 'More HP', 'SP', 'Cure'],
      ),
      _p(
        'Drink with **${_k(binds.potion, 'R')}** (healing) and **${_k(binds.potion2, 'T')}** (SP), or turn on Auto-use potions in the settings. '
        'Potions heal over a few seconds; Green Potions and Green Herbs cure poison and other conditions.',
      ),
      _fig(diagram: 'downed', caption: 'Stand by a downed ally to get them up'),
      _list([
        'At 0 HP you are **downed**: an ally standing next to you for 3 seconds gets you back up. '
            'If nobody comes in 20 seconds, you are out until the next wave.',
        'Alone, being downed ends the run.',
        'A lost run costs a little experience, never a level.',
      ]),
      _fig(diagram: 'fog', caption: 'Get inside the circle'),
      _p('Late in a run the **poison fog** rises and closes in: stay inside the safe circle, it hurts more the longer you stay out.'),
    ]),
    _topic('items', 'Items and trade', 'Gear, shops, refining, cards and trading', 'item:sword', [
      _fig(icons: ['item:sword', 'item:helm', 'item:shield', 'item:boots'], caption: 'Weapon, headgear, shield, shoes'),
      _list([
        'Ten equipment slots: three headgear, armour, weapon, shield, garment, shoes and two accessories. **Double-click** an item to wear or use it.',
        'Shops take a cart: pick quantities (+/−, +10, Max) or drag items in; selling pays half the price.',
        'To trade, right click a player in town: both add items and zeny, press OK, then Trade. Any change unlocks both sides.',
        'Elite and boss drops go to everyone in the party.',
      ]),
      _fig(
        icons: ['item:ember_blade', 'item:night_goggles', 'item:protection_ring'],
        labels: ['Uncommon', 'Uncommon', 'Rare'],
        caption: 'Magic items: green and blue names',
      ),
      _p(
        '**Magic items** drop from elites and bosses (now and then from tougher monsters): a green name is uncommon, a blue one rare. '
        'Besides their plain numbers they carry an enchantment: faster boots, a blade that burns, a wand that makes your spells harder to resist, '
        'a periapt that makes potions heal more. Every class has its own.',
      ),
      _fig(
        icons: ['item:phracon', 'item:emveretarcon', 'item:oridecon', 'item:elunium'],
        labels: ['+1–4', '+5–7', 'Weapon +8–10', 'Armour +8–10'],
        caption: 'Refine ores',
      ),
      _p(
        '**Refining**: Hollgrehenn the Smith, at his forge in the round house north-east of the square in Izlude, hammers a weapon, body armour, shield, hat, garment or shoes '
        'one level up, to +10, for an ore and a fee. A weapon gains damage, armour gains HP (and AC at +4 and +8).',
      ),
      _list([
        '**Phracon** (+1 to +4) and **Emveretarcon** (+5 to +7) are sold by the weapon and armour dealers; **Oridecon** (weapons) and **Elunium** (armour) for +8 to +10 drop from elites and bosses.',
        'Up to the **safe limit** it always works: +4 for armour, +4 to +7 for weapons (the lower the weapon level, the further). Past it the odds fall from 70% to 10%.',
        'A failed refine costs the ore, the fee and a **roll of levels**: d2 up to +5, d4 at +6 and +7, d6 at +8, d8 at +9. A failed +9 can fall all the way to +1. Nothing ever breaks.',
      ]),
      _fig(
        icons: ['item:poring_card', 'item:willow_card', 'item:eclipse_card', 'item:angeling_card'],
        labels: ['Common', 'Uncommon', 'Rare', 'Mythic'],
        caption: 'Monster cards',
      ),
      _p(
        '**Monster cards**: every monster has its own card and only it drops it, rarely (elites more often, bosses for the whole party). '
        'A card fits one kind of equipment and adds its word to the name: a Drops Card makes a **Sword of Kindling** that sets monsters alight, '
        'a Hornet Card a **Stinging** one that hurts small monsters more.',
      ),
      _list([
        'Plain gear has the most **sockets** (up to 4 on a two-handed weapon), magic gear fewer: a carded common build is a match for drops.',
        'Socket and take cards out **for free, in town**: Equipment → **Cards**, or select an item in the inventory and press Cards. Nothing is lost; a carded item cannot be sold until it is empty.',
        'Effects go beyond numbers: setting monsters alight, dodging, a wisp that casts Fire Bolt, a free skill every 20 s, one last stand per expedition. Sets of one family (the slimes, the desert pack, the restless dead) add more.',
        'The **Card Album** (Equipment → Album) keeps every card you have held; double-click a card to look at it and tilt it.',
      ]),
    ]),
    _topic('party', 'Parties', 'Playing together', 'cc:role_support', [
      _fig(diagram: 'party'),
      _list([
        'Up to **8** players. Invite someone (right click them) or join with a **party code** at the Kafra.',
        'The leader picks the expedition and starts it whenever they like; whoever is in town comes along.',
        'The party stays together after a run, when you switch characters and when you log out.',
        'Ping flags (**Ctrl+1–0**) mark a spot for everyone; buffs and heals go to your party too.',
        'Bigger parties meet bigger waves.',
      ]),
    ]),
    _topic('difficulty', 'Difficulty and curses', 'More risk, a bigger reward', 'bonus:zeny', [
      _p(
        'In the expedition window the leader can raise the **difficulty** (Hard, Expert, Nightmare, Hell) and add **curses** '
        'such as Darkness, Frenzy or No Second Chances.',
      ),
      _list([
        'Each step makes monsters tougher, faster and more numerous.',
        'The risk adds to the clear bonus: more zeny, more experience, extra chest rolls. It pays only on a win.',
      ]),
      _tip('Play your level band first: an expedition far below your level pays less experience.'),
    ]),
    _topic('settings', 'Settings', 'Make the game yours', 'cc:axis_control', [
      _list([
        '**Controls**: Manual aiming, Click to move, Tap to move, touch controls.',
        '**Keys**: every binding, your emotes and flags.',
        '**Graphics**: frame rate, effects, damage numbers, the mouse cursor and its size.',
        '**Sound**: music, effects and ambience volumes.',
        '**Interface**: UI scale, full screen, reset window positions.',
      ]),
      _p('Open them with **F10** or from the menu (**Esc**).'),
    ]),
    _topic('rules', 'Rules and glossary', 'Every rules word, explained', 'cc:hit_die', [
      _p(
        'Every underlined word in the game is here. Search, or open a word to read it; lines marked **You** are your own numbers. '
        'The rolls behind them are in **How combat rolls work**.',
      ),
      {'glossary': true},
    ]),
  ];
}

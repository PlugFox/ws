/// Loading-screen tips (`GameView.beginLoading`): the one list of them.
/// General ones are written here; the rest come from what the server sent
/// this session, so they follow the live tables: the mission's bosses and
/// their arrival, its time limit, monster levels, level band and map events
/// (`MissionBrief`), and
/// every mission modifier (`ModifierDefs`). Text may hold `<b>` only; the
/// server's `**bold**` becomes that.
library;

import 'protocol.dart';

/// Keys the general tips name, as the player bound them.
typedef TipKeys = ({String interact, String sit});

/// General tips: [town] ones on the way to a town map, the others on the
/// way to a mission.
List<String> _general(TipKeys keys, {required bool town}) => [
  'Press <b>F10</b> for settings and key bindings.',
  'Emotes are on <b>Alt+1–0</b>, ping flags for the party on <b>Ctrl+1–0</b>.',
  'Spend skill points with <b>K</b> after a level up.',
  if (town) ...[
    'The missions start at the <b>Kafra employee</b> in the town centre.',
    'Talk to people with <b>${keys.interact}</b> or Space. <b>${keys.sit}</b> sits you down.',
    'Players trade with each other in town only.',
    'The party leader picks the mission <b>modifiers</b>: more risk, a bigger clear bonus.',
  ] else ...[
    'Downed? An ally standing by you gets you back up.',
    'When the <b>poison fog</b> rises, stay inside the safe circle.',
    'Monsters outside your <b>light</b> stay hidden until they come close.',
    'A <b>Thief</b> striking out of Hiding or Cloaking rolls with advantage.',
  ],
];

String _clock(int secs) => '${secs ~/ 60}:${(secs % 60).toString().padLeft(2, '0')}';

String _bold(String text) => text.replaceAllMapped(RegExp(r'\*\*(.+?)\*\*'), (m) => '<b>${m[1]}</b>');

/// Tips for a load: [town] or a mission ([mission] its name), from the
/// [brief] of the mission, its card's own tips ([missionTips], server
/// `MissionInfo::tips` via `MISSION_LIST`) and the [modifiers] table when known.
List<String> loadingTips(
  TipKeys keys, {
  required bool town,
  String mission = '',
  MissionBrief? brief,
  List<String> missionTips = const [],
  List<ModifierDef> modifiers = const [],
}) {
  final run = mission.isEmpty ? 'the mission' : mission;
  return [
    ..._general(keys, town: town),
    if (brief != null && !town) ...[
      for (final m in brief.monsters.where((m) => m.archetype == 'boss'))
        '<b>${m.name}</b> shows up <b>${_clock(m.unlockSecs)}</b> into $run.',
      if (brief.timeLimitSecs > 0) 'Hold out for <b>${_clock(brief.timeLimitSecs)}</b> to clear $run.',
      if (brief.levelEverySecs > 0)
        'Monsters here grow a level every <b>${_clock(brief.levelEverySecs)}</b> '
            '(level ${brief.levelMin}–${brief.levelMax}).',
      if (brief.band.min > 1) '$run is made for levels <b>${brief.band.min}–${brief.band.max}</b>.',
      for (final e in brief.events)
        if (e.starts.isNotEmpty)
          e.starts.length > 1
              ? '<b>${e.name}</b> strikes at <b>${_clock(e.starts.first)}</b>, then every '
                    '<b>${_clock(e.starts[1] - e.starts[0])}</b>.'
              : '<b>${e.name}</b> strikes at <b>${_clock(e.starts.first)}</b>.',
    ],
    if (!town) ...missionTips,
    for (final m in modifiers)
      if (m.desc.isNotEmpty) 'Modifier <b>${m.name}</b>: ${_bold(m.desc)}',
  ];
}

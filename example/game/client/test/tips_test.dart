import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/tips.dart';
import 'package:test/test.dart';

MonsterBrief _monster(String name, String archetype, int unlock) => MonsterBrief(
  id: name.toLowerCase(),
  name: name,
  archetype: archetype,
  element: 'water',
  size: 'small',
  unlockSecs: unlock,
  levelMin: 1,
  levelMax: 5,
  hpMin: 10,
  hpMax: 20,
  packMin: 1,
  packMax: 3,
  drops: const [],
);

void main() {
  const keys = (interact: 'E', sit: 'X');

  test('town tips name the bound keys and no mission data', () {
    final tips = loadingTips(keys, town: true);
    expect(tips, contains('Talk to people with <b>E</b> or Space. <b>X</b> sits you down.'));
    expect(tips.where((t) => t.contains('poison fog')), isEmpty);
  });

  test('mission tips come from the briefing', () {
    final withBoss = MissionBrief(
      mission: 'poring_island_survival',
      map: 'prt_fild08',
      timeLimitSecs: 900,
      levelMin: 1,
      levelMax: 12,
      levelEverySecs: 90,
      monsters: [_monster('Poring', 'swarm', 0), _monster('Angeling', 'boss', 600)],
    );
    final tips = loadingTips(keys, town: false, mission: 'Poring Island Survival', brief: withBoss);
    expect(tips, contains('<b>Angeling</b> shows up <b>10:00</b> into Poring Island Survival.'));
    expect(tips, contains('Hold out for <b>15:00</b> to clear Poring Island Survival.'));
    expect(tips, contains('Monsters here grow a level every <b>1:30</b> (level 1–12).'));
    expect(tips.where((t) => t.contains('Poring</b> shows up')), isEmpty, reason: 'only bosses');
    expect(loadingTips(keys, town: true, brief: withBoss).where((t) => t.contains('Angeling')), isEmpty);
  });
}

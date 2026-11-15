import 'package:game_client/src/parties.dart';
import 'package:game_client/src/protocol.dart';
import 'package:test/test.dart';

PartyListing party(
  int id, {
  bool running = false,
  bool mine = false,
  int members = 2,
  int capacity = 4,
}) => PartyListing(
  id,
  'poring',
  'Poring Meadow',
  capacity,
  running ? PartyListing.onMission : PartyListing.forming,
  running ? 125 : 0,
  mine,
  [for (var i = 0; i < members; i++) (name: i == 0 ? 'Ann' : 'P$i', archetype: 'mage', level: 10 + i, online: i == 0)],
);

void main() {
  test('run clock', () {
    expect(runClock(0), '0:00');
    expect(runClock(125), '2:05');
    expect(runClock(3725), '1:02:05');
  });

  test('a party on an expedition says how long and that you go at once', () {
    final o = partyOption(party(3, running: true), inParty: false, meAway: false);
    expect(o['id'], 'parties:join:3');
    expect(o['label'], "Ann's party: Poring Meadow");
    expect(o['detail'], contains('on the expedition for 2:05'));
    expect(o['detail'], contains('Mage 10'));
    expect(o['detail'], contains('land beside a member'));
    expect(o.containsKey('disabled'), isFalse);
  });

  test('full parties, foreign parties while in one, and one\'s own in town are off', () {
    expect(partyOption(party(1, members: 4), inParty: false, meAway: false)['disabled'], isTrue);
    expect(partyOption(party(1), inParty: true, meAway: false)['disabled'], isTrue);
    expect(partyOption(party(1, mine: true), inParty: true, meAway: false)['disabled'], isTrue);
    expect(partyOption(party(1, mine: true, running: true), inParty: true, meAway: true)['disabled'], isTrue);
    final back = partyOption(party(1, mine: true, running: true), inParty: true, meAway: false);
    expect(back.containsKey('disabled'), isFalse, reason: 'back in town early: rejoin the run');
    expect(back['label'], 'Your party: Poring Meadow');
  });

  test('the dialog: an empty list says so, a refresh is always there', () {
    final d = partiesDialog(const []);
    expect(d['text'], contains('No public party'));
    final options = d['options']! as List<Object?>;
    expect(options.cast<Map<String, Object?>>().map((o) => o['id']), ['parties:list']);
  });
}

/// The Kafra's list of public parties (server `lobby.rs`
/// `public_parties`): a dialog of parties to join, in town or already on
/// an expedition (a late join lands beside a member).
library;

import 'package:game_client/src/protocol.dart';

/// `m:ss` (or `h:mm:ss`) of a run's elapsed seconds.
String runClock(int secs) {
  final h = secs ~/ 3600, m = (secs % 3600) ~/ 60, s = secs % 60;
  final ss = s.toString().padLeft(2, '0');
  return h > 0 ? '$h:${m.toString().padLeft(2, '0')}:$ss' : '$m:$ss';
}

String _className(String archetype) =>
    archetype.isEmpty ? 'Novice' : '${archetype[0].toUpperCase()}${archetype.substring(1)}';

/// One option line of a party: who leads, where to, how full, where now.
Map<String, Object?> partyOption(PartyListing p, {required bool inParty, required bool meAway}) {
  final leader = p.members.isEmpty ? 'A' : "${p.members.first.name}'s";
  final where = p.running ? 'on the expedition for ${runClock(p.elapsedSecs)}' : 'gathering in town';
  final who = [
    for (final m in p.members) '${m.name} (${_className(m.archetype)} ${m.level}${m.online ? '' : ', away'})',
  ].join(', ');
  final String? why;
  if (p.mine) {
    why = !p.running
        ? 'Your party is in town.'
        : meAway
        ? 'You are on it already.'
        : null;
  } else if (inParty) {
    why = 'Leave your party first.';
  } else if (p.full) {
    why = 'Full.';
  } else {
    why = null;
  }
  return {
    'id': 'parties:join:${p.lobbyId}',
    'label': '${p.mine ? 'Your party' : '$leader party'}: ${p.missionName}',
    'detail': [
      '${p.members.length}/${p.capacity}, $where.',
      who,
      if (why != null)
        why
      else if (p.running)
        (p.mine ? 'Rejoin the run beside your party.' : 'Join and go: you land beside a member.'),
    ].where((s) => s.isNotEmpty).join(' '),
    if (why != null) 'disabled': true,
  };
}

/// The whole dialog (`GameView.showDialog`); `lobby` is the player's
/// party, `leading` whether they lead it.
Map<String, Object?> partiesDialog(
  List<PartyListing> parties, {
  LobbyInfo? lobby,
  bool leading = false,
  bool meAway = false,
}) => {
  'id': 'parties',
  'title': 'Parties looking for adventurers',
  'text': parties.where((p) => !p.mine).isEmpty
      ? 'No public party is out right now. Pick an expedition and open your party to everyone, or ask again in a little while.'
      : 'Join a party gathering in town, or one already on its expedition: you go at once and land beside a member.',
  'options': [
    for (final p in parties) partyOption(p, inParty: lobby != null, meAway: meAway),
    if (lobby != null && leading)
      lobby.public
          ? {
              'id': 'parties:public:0',
              'label': 'Hide my party from this list',
              'detail': 'Only players with your party code (or an invitation) can join then.',
            }
          : {
              'id': 'parties:public:1',
              'label': 'List my party here',
              'detail': 'Anyone may join it from a Kafra, even while you are on an expedition.',
            },
    {'id': 'parties:list', 'label': 'Refresh', 'detail': 'Ask the Kafra again.'},
  ],
};

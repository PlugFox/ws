import 'dart:convert';

import 'package:game_client/src/changelog.dart';
import 'package:game_client/src/feedback.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/version.dart';
import 'package:test/test.dart';

void main() {
  group('feedback form', () {
    test('the spec carries the choices, the limits and the version', () {
      final f = FeedbackForm(npc: 'scribe', kind: 'balance');
      final spec = jsonDecode(f.spec()) as Map<String, Object?>;
      expect((spec['kind'], spec['priority'], spec['busy']), ('balance', 'normal', false));
      expect([for (final k in spec['kinds']! as List) (k as Map)['id']], ['bug', 'suggestion', 'balance', 'other']);
      expect([for (final p in spec['priorities']! as List) (p as Map)['id']], ['low', 'normal', 'high', 'critical']);
      expect((spec['titleMin'], spec['bodyMax']), (FeedbackLimits.titleMin, FeedbackLimits.bodyMax));
      expect(spec['version'], clientBuild);
      expect(FeedbackForm(npc: 'kafra').kind, 'bug', reason: 'an unknown kind falls back');
    });

    test('short fields stay local; a good form becomes the frame', () {
      final f = FeedbackForm(npc: 'scribe');
      expect(f.submit({'kind': 'bug', 'priority': 'high', 'title': 'x', 'body': 'long enough body text here'}, userAgent: 'UA'), isNull);
      expect(f.error, contains('title'));
      expect(f.submit({'title': 'Stuck', 'body': 'short'}, userAgent: 'UA'), isNull);
      expect(f.error, contains('Describe'));
      expect(f.busy, isFalse);
      final frame = f.submit(
        {'kind': 'suggestion', 'priority': 'low', 'title': '  More   maps ', 'body': ' Please add a desert map with sand worms. '},
        userAgent: 'U' * 400,
      )!;
      expect((f.busy, f.error, f.kind, f.priority), (true, null, 'suggestion', 'low'));
      expect(frame.first, Kind.feedbackReq);
      final text = utf8.decode(frame.sublist(1), allowMalformed: true);
      expect(text, contains('More maps'));
      expect(text, contains('Please add a desert map with sand worms.'));
      expect(text, contains('U' * FeedbackLimits.userAgentMax));
      expect(text, isNot(contains('U' * (FeedbackLimits.userAgentMax + 1))));
      f.refused('Wait a minute.');
      expect((f.busy, f.error), (false, 'Wait a minute.'));
    });
  });

  test('versions', () {
    expect(clientVersion, '0.${changelog.length}');
    expect(clientBuild, startsWith(clientVersion));
    expect(versionLine(null), 'Client $clientBuild');
    expect(versionLine('0.1.0+ab'), 'Client $clientBuild · Server 0.1.0+ab');
  });
}

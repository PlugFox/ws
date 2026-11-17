import 'package:game_client/src/online.dart';
import 'package:game_client/src/update.dart';
import 'package:test/test.dart';

void main() {
  group('update', () {
    test('the update close names the server build', () {
      expect(refusedBy(closeUpdateRequired, 'update_required:1a2b3c4d'), '1a2b3c4d');
      expect(refusedBy(closeUpdateRequired, null), '');
      expect(refusedBy(4008, 'update_required:1a2b3c4d'), isNull);
      expect(refusedBy(null, null), isNull);
    });

    test('only a known, different build is another', () {
      expect(isOtherBuild('1a2b3c4d', client: '1a2b3c4d'), isFalse);
      expect(isOtherBuild('0f0f0f0f', client: '1a2b3c4d'), isTrue);
      expect(isOtherBuild(null, client: '1a2b3c4d'), isFalse, reason: 'an older server says nothing');
      expect(isOtherBuild('', client: 'dev'), isFalse);
      expect(clientBuildId, 'dev', reason: 'tests are built without GAME_BUILD');
    });

    ({UpdateWatch watch, List<Map<String, Object?>?> shown, List<int> reloads, Map<String, String> store}) rig({
      bool storage = true,
    }) {
      final shown = <Map<String, Object?>?>[];
      final reloads = <int>[];
      final store = <String, String>{};
      final watch = UpdateWatch(
        show: shown.add,
        reload: () => reloads.add(1),
        read: (k) => store[k],
        write: (k, v) {
          if (!storage) return false;
          store[k] = v;
          return true;
        },
        client: '1a2b3c4d',
      );
      return (watch: watch, shown: shown, reloads: reloads, store: store);
    }

    test('a refusal reloads once per server build, then shows the modal', () {
      final r = rig();
      r.watch.refused('0f0f0f0f');
      expect(r.reloads, hasLength(1));
      expect(r.shown.single?['required'], isTrue);
      // The reload came back to the same refusal: no loop, the modal.
      final again = rig()..store.addAll(r.store);
      again.watch.refused('0f0f0f0f');
      expect(again.reloads, isEmpty);
      expect(again.shown.single, {'required': true});
      // A newer deploy still: one more reload.
      again.watch.refused('cafebabe');
      expect(again.reloads, hasLength(1));
    });

    test('without storage a refusal never reloads by itself', () {
      final r = rig(storage: false);
      r.watch.refused('0f0f0f0f');
      expect(r.reloads, isEmpty);
      expect(r.shown.single, {'required': true});
    });

    test('a poll naming another build offers the button once', () {
      final r = rig();
      r.watch
        ..sawServer('1a2b3c4d')
        ..sawServer(null);
      expect(r.shown, isEmpty);
      r.watch
        ..sawServer('0f0f0f0f')
        ..sawServer('0f0f0f0f');
      expect(r.shown, [<String, Object?>{}]);
      expect((r.watch.offered, r.watch.required), (true, false));
      expect(r.reloads, isEmpty, reason: 'never interrupts play');
    });

    test('the online poll reads the build', () async {
      OnlineStatus? got;
      final poller = OnlinePoller(() async => {'online': 3, 'build': '0f0f0f0f'}, (s) => got = s)..start();
      await Future<void>.delayed(Duration.zero);
      poller.stop();
      expect((got?.online, got?.build), (3, '0f0f0f0f'));
    });
  });
}

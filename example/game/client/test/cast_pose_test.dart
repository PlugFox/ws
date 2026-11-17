import 'package:game_client/src/cast_pose.dart';
import 'package:test/test.dart';

void main() {
  group('cast pose', () {
    test('a thrown skill winds up, then tosses as it lands', () {
      final p = CastPose.of('acid_terror', 10, 500);
      expect(p.throws, isTrue);
      expect(p.at(10.2), CastPhase.windUp);
      expect(p.at(10.6), CastPhase.release);
      expect(p.at(10.95), CastPhase.none);
      expect(p.end, closeTo(10.9, 1e-9));
    });

    test('an instant throw tosses at once', () {
      final p = CastPose.of('potion_pitcher', 0, 0);
      expect(p.at(0), CastPhase.release);
      expect(p.at(throwSecs), CastPhase.none);
    });

    test('every alchemist throw tosses; other skills only wind up, a moment at least', () {
      for (final s in ['acid_demonstration', 'demonstration', 'slim_potion_pitcher', 'sphere_mine', 'summon_flora', 'plant_cultivation']) {
        expect(CastPose.of(s, 0, 300).throws, isTrue, reason: s);
      }
      final bolt = CastPose.of('fire_bolt', 0, 0);
      expect(bolt.throws, isFalse);
      expect(bolt.at(0.3), CastPhase.windUp);
      expect(bolt.at(minPoseSecs), CastPhase.none);
    });
  });
}

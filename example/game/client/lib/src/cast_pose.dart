/// What a character's body does for a cast (`HubSession._charAction`): the
/// wind-up pose (the sprite's `cast` action) for the cast time, then, for a
/// skill thrown from the hand, the throw itself as it lands (the sprite's
/// `attack` action: the overhand toss every body has, drawn without the
/// weapon). Instant throws skip the wind-up.
library;

import 'dart:math' as math;

/// Skills thrown from the hand: the alchemist's flasks and potions, the
/// Marine Sphere and the plant bottles of Summon Flora and Plant
/// Cultivation (skills.toml ids).
const thrownSkills = {
  'acid_terror',
  'acid_demonstration',
  'demonstration',
  'potion_pitcher',
  'slim_potion_pitcher',
  'sphere_mine',
  'summon_flora',
  'plant_cultivation',
};

/// Seconds the throw shows (the toss is 5 frames of 60 ms, then a beat).
const throwSecs = 0.4;

/// Seconds an instant skill (not thrown) holds the wind-up pose at least.
const minPoseSecs = 0.4;

/// Where a cast's pose is at a moment.
enum CastPhase { none, windUp, release }

/// The pose of one cast pressed at `start` (seconds on the session clock).
final class CastPose {
  /// End of the wind-up pose.
  final double windUpEnd;

  /// End of the throw (== [windUpEnd]: no throw).
  final double releaseEnd;

  const CastPose._(this.windUpEnd, this.releaseEnd);

  factory CastPose.of(String skill, double start, int castMs) {
    final windUp = math.max(castMs, 0) / 1000;
    if (thrownSkills.contains(skill)) return CastPose._(start + windUp, start + windUp + throwSecs);
    final end = start + math.max(windUp, minPoseSecs);
    return CastPose._(end, end);
  }

  bool get throws => releaseEnd > windUpEnd;

  /// When the whole pose is over.
  double get end => math.max(windUpEnd, releaseEnd);

  CastPhase at(double now) => now < windUpEnd
      ? CastPhase.windUp
      : now < releaseEnd
      ? CastPhase.release
      : CastPhase.none;
}

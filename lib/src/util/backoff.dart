import 'dart:math' as math;

/// Exponential backoff with full jitter:
/// `min + random(0, min(max - min, min * 2^attempt))`.
///
/// https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/
Duration backoffDelay(
  int attempt,
  Duration min,
  Duration max, [
  math.Random? random,
]) {
  final lo = min.inMicroseconds;
  final hi = max.inMicroseconds;
  if (lo >= hi) return max;
  // A zero minimum must still grow (100 ms base), or every retry is
  // immediate.
  final base = math.max(lo, _minBase);
  final ceiling = math.min(hi - lo, base * math.pow(2, attempt.clamp(0, 31)));
  final jitter = ceiling <= 0
      ? 0
      : ((random ?? _random).nextDouble() * ceiling).floor();
  return Duration(microseconds: lo + jitter);
}

final math.Random _random = math.Random();

const int _minBase = 100000; // µs

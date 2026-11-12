import 'dart:math' as math;

/// Maps server ticks to local time and picks an adaptive render delay.
///
/// Each snapshot gives a sample `offset = arrival - tick * tickDt`. The
/// estimate follows decreases at once (a faster packet is a better bound on
/// the transit time) and creeps up slowly, so a late burst does not drag the
/// timeline. Jitter is the mean deviation from the estimate; the render
/// delay is roughly 100 ms at 20 Hz and grows with jitter so the buffer
/// almost always holds a sample on both sides of the render time.
final class ServerClock {
  ServerClock({required this.tickHz});

  final int tickHz;
  double? _offset;
  double jitter = 0;
  double delay = 0.1;

  /// Extra render delay from settings (seconds); trades latency for fewer
  /// extrapolation stalls on bad links.
  double bias = 0;

  double get tickDt => 1 / tickHz;
  bool get synced => _offset != null;

  void reset() {
    _offset = null;
    jitter = 0;
  }

  /// [arrival] in seconds of local monotonic time.
  void onSnapshot(int tick, double arrival) {
    final sample = arrival - tick * tickDt;
    final o = _offset;
    if (o == null) {
      _offset = sample;
      return;
    }
    final dev = sample - o;
    jitter += (dev.abs() - jitter) * 0.05;
    _offset = dev < 0 ? sample : o + dev * 0.01;
    final target = (tickDt * 1.5 + jitter * 4).clamp(0.075, 0.3);
    delay += (target - delay) * 0.05;
  }

  /// Server tick (fractional) to render remote entities at.
  double renderTick(double now) {
    final o = _offset;
    if (o == null) return 0;
    return (now - o - delay - bias) / tickDt;
  }
}

/// One remote entity's recent authoritative samples.
final class RemoteTrack {
  RemoteTrack(this.id);

  final int id;
  static const int capacity = 32;
  final List<int> _ticks = List.filled(capacity, 0);
  final List<double> _xs = List.filled(capacity, 0);
  final List<double> _ys = List.filled(capacity, 0);
  final List<int> _facings = List.filled(capacity, 0);
  int _head = 0;
  int _len = 0;

  // Last interpolated state.
  double x = 0;
  double y = 0;
  int facing = 4;
  bool moving = false;

  /// Samples further apart than this are teleports (portals): no blending.
  static const double teleport = 3;

  /// Extrapolate at most this many ticks past the newest sample, then hold.
  static const double maxExtrapolationTicks = 2;

  void push(int tick, double x, double y, int facing) {
    if (_len > 0) {
      final last = (_head - 1 + capacity) % capacity;
      if (tick <= _ticks[last]) return; // duplicate or reordered
      final dx = x - _xs[last], dy = y - _ys[last];
      if (dx * dx + dy * dy > teleport * teleport) _len = 0;
    }
    _ticks[_head] = tick;
    _xs[_head] = x;
    _ys[_head] = y;
    _facings[_head] = facing;
    _head = (_head + 1) % capacity;
    _len = math.min(_len + 1, capacity);
  }

  bool get isEmpty => _len == 0;

  // Current movement segment, set by [segment]: the renderer interpolates
  // from (x0, y0) at tick0 to (x1, y1) at tick1 and may extrapolate up to
  // [maxExtrapolationTicks] past tick1.
  double x0 = 0;
  double y0 = 0;
  double x1 = 0;
  double y1 = 0;
  int tick0 = 0;
  int tick1 = 0;

  // What was last handed to the renderer (owned by the session).
  int sentTick0 = -1;
  int sentTick1 = -1;
  int sentFacing = -1;
  int sentAction = -1;
  int sentSprite = -1;
  double sentX = double.nan;
  double sentY = double.nan;

  /// Forces the next frame to resend this entity (after a map load).
  void resetSent() {
    sentTick0 = sentTick1 = sentFacing = sentAction = sentSprite = -1;
    sentX = sentY = double.nan;
  }

  /// Picks the sample pair bracketing fractional tick [t]. Past the newest
  /// sample the last pair is kept (the shader extrapolates along it); with
  /// one sample, or before the oldest, the segment is a point.
  void segment(double t) {
    if (_len == 0) {
      x0 = x1 = x;
      y0 = y1 = y;
      tick0 = tick1 = 0;
      moving = false;
      return;
    }
    int at(int k) => (_head - _len + k + capacity) % capacity; // k-th oldest
    void point(int i) {
      x0 = x1 = x = _xs[i];
      y0 = y1 = y = _ys[i];
      tick0 = tick1 = _ticks[i];
      facing = _facings[i];
      moving = false;
    }

    void pair(int a, int b) {
      x0 = _xs[a];
      y0 = _ys[a];
      x1 = x = _xs[b];
      y1 = y = _ys[b];
      tick0 = _ticks[a];
      tick1 = _ticks[b];
      facing = _facings[b];
    }

    final newest = at(_len - 1);
    if (_len == 1) return point(newest);
    if (t >= _ticks[newest]) {
      pair(at(_len - 2), newest);
      moving = (x0 != x1 || y0 != y1) && t - _ticks[newest] < maxExtrapolationTicks;
      return;
    }
    final oldest = at(0);
    if (t <= _ticks[oldest]) return point(oldest);
    for (var k = _len - 2; k >= 0; k--) {
      final a = at(k), b = at(k + 1);
      if (_ticks[a] <= t) {
        pair(a, b);
        moving = x0 != x1 || y0 != y1;
        return;
      }
    }
  }

  /// Samples the track at a fractional server tick.
  void sample(double t) {
    if (_len == 0) return;
    int at(int k) => (_head - _len + k + capacity) % capacity; // k-th oldest
    final newest = at(_len - 1);
    if (_len == 1 || t >= _ticks[newest]) {
      if (_len >= 2) {
        final prev = at(_len - 2);
        final span = (_ticks[newest] - _ticks[prev]).toDouble();
        final ahead = math.min(t - _ticks[newest], maxExtrapolationTicks);
        final vx = (_xs[newest] - _xs[prev]) / span, vy = (_ys[newest] - _ys[prev]) / span;
        x = _xs[newest] + vx * ahead;
        y = _ys[newest] + vy * ahead;
        moving = (vx != 0 || vy != 0) && t - _ticks[newest] < maxExtrapolationTicks;
      } else {
        x = _xs[newest];
        y = _ys[newest];
        moving = false;
      }
      facing = _facings[newest];
      return;
    }
    final oldest = at(0);
    if (t <= _ticks[oldest]) {
      x = _xs[oldest];
      y = _ys[oldest];
      facing = _facings[oldest];
      moving = false;
      return;
    }
    for (var k = _len - 2; k >= 0; k--) {
      final a = at(k), b = at(k + 1);
      if (_ticks[a] <= t) {
        final u = (t - _ticks[a]) / (_ticks[b] - _ticks[a]);
        x = _xs[a] + (_xs[b] - _xs[a]) * u;
        y = _ys[a] + (_ys[b] - _ys[a]) * u;
        facing = u < 0.5 ? _facings[a] : _facings[b];
        moving = _xs[a] != _xs[b] || _ys[a] != _ys[b];
        return;
      }
    }
  }
}

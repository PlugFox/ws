import 'dart:async';

import 'stats.dart';

/// Measures how much of the main isolate's time is taken by other work.
///
/// The probe keeps the event loop busy with short spin chunks scheduled via
/// `Timer.run`. Whatever time is not spent spinning was consumed by other
/// events (socket reads, port messages, decoding...). Gaps between chunks
/// show how long the event loop was blocked at once — a proxy for UI jank.
final class MainIsolateProbe {
  MainIsolateProbe({this.chunkMicros = 100});

  final int chunkMicros;
  final Stopwatch _sw = Stopwatch();
  final List<int> _gaps = <int>[];
  int _spun = 0;
  int _lastEnd = 0;
  bool _running = false;

  void start() {
    _gaps.clear();
    _spun = 0;
    _running = true;
    _sw
      ..reset()
      ..start();
    _lastEnd = 0;
    Timer.run(_tick);
  }

  void _tick() {
    if (!_running) return;
    final start = _sw.elapsedMicroseconds;
    _gaps.add(start - _lastEnd);
    while (_sw.elapsedMicroseconds - start < chunkMicros) {}
    _lastEnd = _sw.elapsedMicroseconds;
    _spun += _lastEnd - start;
    Timer.run(_tick);
  }

  ProbeResult stop() {
    _running = false;
    _sw.stop();
    final wall = _sw.elapsedMicroseconds;
    return ProbeResult(
      wallMicros: wall,
      spunMicros: _spun,
      busyFraction: wall == 0 ? 0 : 1 - _spun / wall,
      gaps: Percentiles.of(_gaps),
    );
  }

  /// Runs the probe on an idle isolate to measure its own overhead.
  Future<ProbeResult> calibrate(Duration duration) async {
    start();
    await Future<void>.delayed(duration);
    return stop();
  }
}

final class ProbeResult {
  const ProbeResult({
    required this.wallMicros,
    required this.spunMicros,
    required this.busyFraction,
    required this.gaps,
  });

  final int wallMicros;

  /// CPU time burnt by the probe itself (to subtract from process CPU).
  final int spunMicros;

  /// Fraction of wall time the main isolate spent on something other than
  /// the probe (including the probe's own scheduling overhead).
  final double busyFraction;

  /// Distribution of event-loop blocking intervals, µs.
  final Percentiles gaps;
}

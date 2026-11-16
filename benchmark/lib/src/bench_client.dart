/// Platform-neutral parts of the harness: the client interface the
/// scenarios drive and the hooks a platform (VM, Flutter, browser) supplies.
library;

/// A connected benchmark client. Messages are delivered on the main isolate
/// as `Map<String, Object?>` (JSON text) or `Uint8List` (binary).
abstract interface class BenchClient {
  /// Sends a JSON map as a text message or a `Uint8List` as binary.
  void send(Object message);

  Future<void> get done;

  int? get closeCode;

  Future<void> close();
}

/// Opens a [BenchClient] for the variant id.
typedef BenchConnector = Future<BenchClient> Function(
  String variant,
  Uri uri, {
  required void Function(Object message) onMessage,
  required String format,
});

/// What the running platform can measure.
final class BenchPlatform {
  const BenchPlatform({
    required this.connect,
    this.cpuMicros,
    this.maxRssBytes,
    this.probe = true,
  });

  final BenchConnector connect;

  /// Process CPU time (user + system, all threads), µs. `null` in browsers.
  final int Function()? cpuMicros;

  /// Peak resident set size, bytes. `null` in browsers.
  final int Function()? maxRssBytes;

  /// Whether the main-isolate probe works. Browsers clamp nested
  /// `setTimeout(0)` to 4 ms, so the probe would report the clamp, not the
  /// transport.
  final bool probe;
}

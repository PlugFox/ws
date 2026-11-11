import 'dart:ffi';
import 'dart:io';
import 'dart:typed_data';

@Native<Int Function(Int, Pointer<Uint8>)>(symbol: 'getrusage', isLeaf: true)
external int _getrusage(int who, Pointer<Uint8> usage);

/// Process resource usage via `getrusage(RUSAGE_SELF)` (macOS / Linux).
final class ResourceUsage {
  const ResourceUsage(this.cpuMicros, this.maxRssBytes);

  /// User + system CPU time of the whole process (all isolates and threads).
  final int cpuMicros;
  final int maxRssBytes;

  static ResourceUsage now() {
    // struct rusage: timeval ru_utime; timeval ru_stime; long ru_maxrss; ...
    final buf = Uint8List(256);
    if (_getrusage(0, buf.address) != 0) return const ResourceUsage(0, 0);
    final data = ByteData.sublistView(buf);
    int timeval(int offset) =>
        data.getInt64(offset, Endian.host) * 1000000 + data.getInt32(offset + 8, Endian.host);
    final rss = data.getInt64(32, Endian.host);
    return ResourceUsage(
      timeval(0) + timeval(16),
      Platform.isMacOS ? rss : rss * 1024,
    );
  }
}

import 'dart:math' as math;
import 'dart:typed_data';

/// Binary hub snapshot (server → client), little-endian:
/// `u8 kind=0x01, u16 map_epoch, u32 tick, u32 ack_seq, u16 count`, then
/// `count × {u32 entity_id, u16 x, u16 y, u8 facing}` with positions in
/// cells as fixed point (6 fractional bits).
final class Snapshot {
  Snapshot(this.mapEpoch, this.tick, this.ackSeq, this.ids, this.xs, this.ys, this.facings);

  static const int kind = 0x01;
  static const int headerLength = 13;
  static const int entryLength = 9;

  final int mapEpoch;
  final int tick;
  final int ackSeq;
  final Uint32List ids;
  final Float32List xs;
  final Float32List ys;
  final Uint8List facings;

  int get length => ids.length;

  /// Returns null for anything that is not a well-formed snapshot.
  static Snapshot? decode(Uint8List b) {
    if (b.length < headerLength || b[0] != kind) return null;
    final d = ByteData.sublistView(b);
    final count = d.getUint16(11, Endian.little);
    if (b.length != headerLength + count * entryLength) return null;
    final ids = Uint32List(count);
    final xs = Float32List(count);
    final ys = Float32List(count);
    final facings = Uint8List(count);
    for (var i = 0; i < count; i++) {
      final o = headerLength + i * entryLength;
      ids[i] = d.getUint32(o, Endian.little);
      xs[i] = d.getUint16(o + 4, Endian.little) / 64;
      ys[i] = d.getUint16(o + 6, Endian.little) / 64;
      facings[i] = b[o + 8];
    }
    return Snapshot(
      d.getUint16(1, Endian.little),
      d.getUint32(3, Endian.little),
      d.getUint32(7, Endian.little),
      ids,
      xs,
      ys,
      facings,
    );
  }
}

/// Movement input (client → server): `u8 kind=0x02, u32 seq, i16 dx, i16 dy`
/// with the direction scaled by 32767.
Uint8List encodeInput(int seq, double dx, double dy) {
  final d = ByteData(9)
    ..setUint8(0, 0x02)
    ..setUint32(1, seq, Endian.little)
    ..setInt16(5, (dx.clamp(-1.0, 1.0) * 32767).round(), Endian.little)
    ..setInt16(7, (dy.clamp(-1.0, 1.0) * 32767).round(), Endian.little);
  return d.buffer.asUint8List();
}

/// Inverse of the wire quantisation, so prediction uses exactly the
/// direction the server will see.
(double, double) quantizeDir(double dx, double dy) =>
    ((dx.clamp(-1.0, 1.0) * 32767).round() / 32767, (dy.clamp(-1.0, 1.0) * 32767).round() / 32767);

/// 8-way facing from a direction, 0 = north (+y), clockwise (same as server).
int facingOf(double dx, double dy) {
  final step = (math.atan2(dx, dy) / (math.pi / 4)).round();
  return ((step % 8) + 8) % 8;
}

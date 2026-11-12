import 'dart:math' as math;
import 'dart:typed_data';

/// Binary hub snapshot (server → client), little-endian:
/// `u8 kind=0x01, u16 map_epoch, u32 tick, u32 ack_seq, u16 count`, then
/// `count × {u32 entity_id, u16 x, u16 y, u8 facing}` with positions in
/// cells as fixed point (6 fractional bits).
///
/// Stage snapshots (`kind=0x03`) append `u16 monster_count` and
/// `monster_count × {u16 slot, u8 generation, u8 kind, u16 x, u16 y}`:
/// `(slot, generation)` identifies a monster, `kind` indexes
/// `stage_joined.monsters`.
final class Snapshot {
  Snapshot(
    this.mapEpoch,
    this.tick,
    this.ackSeq,
    this.ids,
    this.xs,
    this.ys,
    this.facings,
    this.mobSlots,
    this.mobGenerations,
    this.mobKinds,
    this.mobXs,
    this.mobYs,
  );

  static const int kind = 0x01;
  static const int stageKind = 0x03;
  static const int headerLength = 13;
  static const int entryLength = 9;
  static const int monsterEntryLength = 8;

  static bool isSnapshot(int firstByte) => firstByte == kind || firstByte == stageKind;

  final int mapEpoch;
  final int tick;
  final int ackSeq;
  final Uint32List ids;
  final Float32List xs;
  final Float32List ys;
  final Uint8List facings;
  final Uint16List mobSlots;
  final Uint8List mobGenerations;
  final Uint8List mobKinds;
  final Float32List mobXs;
  final Float32List mobYs;

  int get length => ids.length;
  int get monsterCount => mobSlots.length;

  /// Returns null for anything that is not a well-formed snapshot.
  static Snapshot? decode(Uint8List b) {
    if (b.length < headerLength || !isSnapshot(b[0])) return null;
    final d = ByteData.sublistView(b);
    final count = d.getUint16(11, Endian.little);
    final playersEnd = headerLength + count * entryLength;
    var mobs = 0;
    if (b[0] == stageKind) {
      if (b.length < playersEnd + 2) return null;
      mobs = d.getUint16(playersEnd, Endian.little);
      if (b.length != playersEnd + 2 + mobs * monsterEntryLength) return null;
    } else if (b.length != playersEnd) {
      return null;
    }
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
    final mobSlots = Uint16List(mobs);
    final mobGenerations = Uint8List(mobs);
    final mobKinds = Uint8List(mobs);
    final mobXs = Float32List(mobs);
    final mobYs = Float32List(mobs);
    for (var i = 0; i < mobs; i++) {
      final o = playersEnd + 2 + i * monsterEntryLength;
      mobSlots[i] = d.getUint16(o, Endian.little);
      mobGenerations[i] = b[o + 2];
      mobKinds[i] = b[o + 3];
      mobXs[i] = d.getUint16(o + 4, Endian.little) / 64;
      mobYs[i] = d.getUint16(o + 6, Endian.little) / 64;
    }
    return Snapshot(
      d.getUint16(1, Endian.little),
      d.getUint32(3, Endian.little),
      d.getUint32(7, Endian.little),
      ids,
      xs,
      ys,
      facings,
      mobSlots,
      mobGenerations,
      mobKinds,
      mobXs,
      mobYs,
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

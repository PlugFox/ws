import 'dart:typed_data';

/// `walkability.bin`: `u32 LE width, u32 LE height`, then one byte per cell,
/// 0 = walkable. Index `y * width + x`, `y` grows north.
final class WalkGrid {
  WalkGrid(this.width, this.height, this.cells);

  factory WalkGrid.parse(Uint8List bytes) {
    final d = ByteData.sublistView(bytes);
    final w = d.getUint32(0, Endian.little);
    final h = d.getUint32(4, Endian.little);
    if (bytes.length != 8 + w * h) {
      throw FormatException('walkability.bin: expected ${8 + w * h} bytes, got ${bytes.length}');
    }
    return WalkGrid(w, h, Uint8List.sublistView(bytes, 8));
  }

  /// North row first, `#` blocked (handy for tests).
  factory WalkGrid.ascii(List<String> rows) {
    final h = rows.length, w = rows.first.length;
    final cells = Uint8List(w * h);
    for (var r = 0; r < h; r++) {
      final y = h - 1 - r;
      for (var x = 0; x < w; x++) {
        cells[y * w + x] = rows[r][x] == '#' ? 1 : 0;
      }
    }
    return WalkGrid(w, h, cells);
  }

  final int width;
  final int height;
  final Uint8List cells;

  bool isWalkableCell(int x, int y) => x >= 0 && y >= 0 && x < width && y < height && cells[y * width + x] == 0;

  /// Same rule as the server: the cell containing the point (floor).
  bool isWalkable(double x, double y) => isWalkableCell(x.floor(), y.floor());
}

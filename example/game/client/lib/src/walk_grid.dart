import 'dart:math' as math;
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

  /// Way from (sx, sy) to within [reach] of (tx, ty) (click to walk to an
  /// NPC): A* over walkable cells, 8-way without cutting corners, then
  /// straightened where the line between two points is clear. Returns the
  /// points to walk through (cell centres, the last one in reach; empty
  /// when already there), or null when no walkable cell in reach can be
  /// reached within [budget] expanded cells.
  List<(double, double)>? findPath(double sx, double sy, double tx, double ty, double reach, {int budget = 60000}) {
    if (_near(sx, sy, tx, ty, reach)) return const [];
    final x0 = sx.floor(), y0 = sy.floor();
    if (!isWalkableCell(x0, y0)) return null;
    final n = width * height;
    final g = Float64List(n)..fillRange(0, n, double.infinity);
    final from = Int32List(n)..fillRange(0, n, -1);
    final closed = Uint8List(n);
    // Binary heap of (f, cell).
    final heapF = <double>[];
    final heapC = <int>[];
    void swap(int a, int b) {
      final f = heapF[a], c = heapC[a];
      heapF[a] = heapF[b];
      heapC[a] = heapC[b];
      heapF[b] = f;
      heapC[b] = c;
    }

    void push(double f, int c) {
      heapF.add(f);
      heapC.add(c);
      var i = heapF.length - 1;
      while (i > 0) {
        final p = (i - 1) >> 1;
        if (heapF[p] <= heapF[i]) break;
        swap(p, i);
        i = p;
      }
    }

    int pop() {
      final top = heapC[0];
      final lastF = heapF.removeLast(), lastC = heapC.removeLast();
      if (heapF.isNotEmpty) {
        heapF[0] = lastF;
        heapC[0] = lastC;
        var i = 0;
        for (;;) {
          final l = 2 * i + 1, r = l + 1;
          var m = i;
          if (l < heapF.length && heapF[l] < heapF[m]) m = l;
          if (r < heapF.length && heapF[r] < heapF[m]) m = r;
          if (m == i) break;
          swap(m, i);
          i = m;
        }
      }
      return top;
    }

    double h(int x, int y) {
      final dx = (x + 0.5 - tx).abs(), dy = (y + 0.5 - ty).abs();
      return math.max(0, math.max(dx, dy) + (math.sqrt2 - 1) * math.min(dx, dy) - reach);
    }

    final start = y0 * width + x0;
    g[start] = 0;
    push(h(x0, y0), start);
    var goal = -1;
    var expanded = 0;
    while (heapC.isNotEmpty && expanded < budget) {
      final c = pop();
      if (closed[c] != 0) continue;
      closed[c] = 1;
      expanded++;
      final cx = c % width, cy = c ~/ width;
      if (_near(cx + 0.5, cy + 0.5, tx, ty, reach)) {
        goal = c;
        break;
      }
      for (var dy = -1; dy <= 1; dy++) {
        for (var dx = -1; dx <= 1; dx++) {
          if (dx == 0 && dy == 0) continue;
          final nx = cx + dx, ny = cy + dy;
          if (!isWalkableCell(nx, ny)) continue;
          if (dx != 0 && dy != 0 && (!isWalkableCell(cx + dx, cy) || !isWalkableCell(cx, cy + dy))) continue;
          final nc = ny * width + nx;
          if (closed[nc] != 0) continue;
          final ng = g[c] + (dx != 0 && dy != 0 ? math.sqrt2 : 1);
          if (ng < g[nc]) {
            g[nc] = ng;
            from[nc] = c;
            push(ng + h(nx, ny), nc);
          }
        }
      }
    }
    if (goal < 0) return null;
    final cells = <(double, double)>[];
    for (var c = goal; c != start; c = from[c]) {
      cells.add(((c % width) + 0.5, (c ~/ width) + 0.5));
    }
    final raw = cells.reversed.toList();
    // Straighten: from each point, jump to the farthest one in clear view.
    final out = <(double, double)>[];
    var (ax, ay) = (sx, sy);
    var i = 0;
    while (i < raw.length) {
      var j = raw.length - 1;
      while (j > i && !clearLine(ax, ay, raw[j].$1, raw[j].$2)) {
        j--;
      }
      out.add(raw[j]);
      (ax, ay) = raw[j];
      i = j + 1;
    }
    return out;
  }

  static bool _near(double x, double y, double tx, double ty, double reach) {
    final dx = x - tx, dy = y - ty;
    return dx * dx + dy * dy <= reach * reach;
  }

  /// Whether a body (about 0.6 cells wide) walks straight from (x0, y0) to
  /// (x1, y1) over walkable cells only.
  bool clearLine(double x0, double y0, double x1, double y1) {
    final dx = x1 - x0, dy = y1 - y0;
    final len = math.sqrt(dx * dx + dy * dy);
    if (len < 1e-6) return isWalkable(x0, y0);
    final steps = (len / 0.25).ceil();
    final nx = -dy / len * 0.3, ny = dx / len * 0.3;
    for (var s = 0; s <= steps; s++) {
      final t = s / steps;
      final x = x0 + dx * t, y = y0 + dy * t;
      if (!isWalkable(x, y) || !isWalkable(x + nx, y + ny) || !isWalkable(x - nx, y - ny)) return false;
    }
    return true;
  }
}

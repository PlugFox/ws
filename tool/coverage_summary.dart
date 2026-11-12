// Prints per-file and total line coverage from an lcov file, lowest first.
// Usage: dart run tool/coverage_summary.dart coverage/lcov.info
import 'dart:io';

void main(List<String> args) {
  final path = args.isEmpty ? 'coverage/lcov.info' : args.first;
  final found = <String, int>{};
  final hit = <String, int>{};
  String? file;
  for (final line in File(path).readAsLinesSync()) {
    if (line.startsWith('SF:')) {
      file = line.substring(3).replaceFirst(RegExp('^.*?/lib/'), 'lib/');
    } else if (line.startsWith('LF:')) {
      found[file!] = int.parse(line.substring(3));
    } else if (line.startsWith('LH:')) {
      hit[file!] = int.parse(line.substring(3));
    }
  }
  final files = found.keys.toList()
    ..sort((a, b) => (hit[a]! / found[a]!).compareTo(hit[b]! / found[b]!));
  var totalFound = 0;
  var totalHit = 0;
  for (final f in files) {
    totalFound += found[f]!;
    totalHit += hit[f]!;
    stdout.writeln(_row(hit[f]!, found[f]!, f));
  }
  stdout.writeln(_row(totalHit, totalFound, 'TOTAL'));
}

String _row(int hit, int found, String name) {
  final percent = found == 0 ? 100.0 : 100 * hit / found;
  return '${percent.toStringAsFixed(1).padLeft(5)}%  '
      '${'$hit/$found'.padLeft(9)}  $name';
}

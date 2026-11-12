// Writes native/prebuilt/checksums.json: SHA-256 of every library under
// native/prebuilt/<target>/, keyed `<target>/<file>`. The build hook checks
// bundled libraries against it, and tool/native_release.dart copies it into
// package:ws for `dart run ws:install` (DECISIONS.md D9, D32).
//
//   dart run tool/native_checksums.dart [native/prebuilt]
import 'dart:convert';
import 'dart:io';

import 'package:ws/src/native/sha256.dart';

void main(List<String> args) {
  final dir = Directory(args.isEmpty ? 'native/prebuilt' : args.first);
  final hashes = <String, String>{};
  for (final target in dir.listSync().whereType<Directory>()) {
    for (final file in target.listSync().whereType<File>()) {
      final name = file.uri.pathSegments.last;
      if (!name.contains('ws_native')) continue;
      final key =
          '${target.uri.pathSegments.lastWhere((s) => s.isNotEmpty)}'
          '/$name';
      hashes[key] = (Sha256()..add(file.readAsBytesSync())).close();
    }
  }
  final sorted = {for (final k in hashes.keys.toList()..sort()) k: hashes[k]};
  File('${dir.path}/checksums.json').writeAsStringSync(
    '${const JsonEncoder.withIndent('  ').convert(sorted)}\n',
  );
  stdout.writeln('checksums.json: ${hashes.length} libraries');
}

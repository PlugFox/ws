// Writes native/prebuilt/checksums.json: SHA-256 of every library under
// native/prebuilt/<target>/, keyed `<target>/<file>`. The build hook checks
// bundled libraries against it, and tool/native_release.dart copies it into
// package:ws for `dart run ws:install` (DECISIONS.md D9, D32).
//
//   dart run tool/native_checksums.dart [native/prebuilt]
//   dart run tool/native_checksums.dart --check [native/prebuilt]
//
// --check writes nothing: it fails unless checksums.json lists exactly the
// 12 release targets and matches the files on disk, so a library rebuilt
// or modified locally is never blessed silently (distribution.md F7).
import 'dart:convert';
import 'dart:io';

import 'package:ws/src/native/install.dart' show nativeLibraryFileName;
import 'package:ws/src/util/sha256.dart';

/// Every target a release ships (tool/build_native.sh, prebuilt.yml).
const releaseTargets = [
  'android-arm',
  'android-arm64',
  'android-x64',
  'ios-arm64',
  'ios-arm64-sim',
  'ios-x64-sim',
  'linux-arm64',
  'linux-x64',
  'macos-arm64',
  'macos-x64',
  'windows-arm64',
  'windows-x64',
];

void main(List<String> args) {
  final check = args.contains('--check');
  final paths = args.where((a) => !a.startsWith('--'));
  final dir = Directory(paths.isEmpty ? 'native/prebuilt' : paths.first);
  if (!dir.existsSync()) {
    stderr.writeln('native_checksums: ${dir.path} does not exist');
    exit(1);
  }
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
  final json = File('${dir.path}/checksums.json');
  if (!check) {
    json.writeAsStringSync(
      '${const JsonEncoder.withIndent('  ').convert(sorted)}\n',
    );
    stdout.writeln('checksums.json: ${hashes.length} libraries');
    return;
  }
  final errors = <String>[];
  final expected = {
    for (final t in releaseTargets) '$t/${nativeLibraryFileName(t)}',
  };
  final listed = json.existsSync()
      ? (jsonDecode(json.readAsStringSync()) as Map<String, Object?>)
      : const <String, Object?>{};
  if (!json.existsSync()) errors.add('${json.path} is missing');
  for (final key in expected.difference(listed.keys.toSet())) {
    errors.add('$key: not in checksums.json');
  }
  for (final key in listed.keys.toSet().difference(expected)) {
    errors.add('$key: not a release target');
  }
  for (final key in hashes.keys.toSet().difference(listed.keys.toSet())) {
    errors.add('$key: on disk but not in checksums.json');
  }
  for (final MapEntry(:key, :value) in listed.entries) {
    final actual = hashes[key];
    if (actual == null) {
      if (expected.contains(key)) errors.add('$key: file is missing');
    } else if (actual != value) {
      errors.add('$key: hash differs from checksums.json');
    }
  }
  if (errors.isNotEmpty) {
    stderr.writeln('native_checksums --check ${dir.path}:');
    for (final e in errors) {
      stderr.writeln('  $e');
    }
    exit(1);
  }
  stdout.writeln('checksums.json: ${listed.length} libraries verified');
}

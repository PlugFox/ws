// Checks that a release tag matches the sources before anything is built
// (distribution.md F7): the version of ws and of ws_native, `wsVersion` in
// lib/src/native/native_release.dart, and the first heading of both
// CHANGELOG.md files must all equal the tag.
//
//   dart run tool/release_check.dart <tag> [repository root]
import 'dart:io';

void main(List<String> args) {
  if (args.isEmpty) {
    stderr.writeln('usage: dart run tool/release_check.dart <tag> [root]');
    exit(2);
  }
  final errors = releaseErrors(args.first, args.length > 1 ? args[1] : '.');
  if (errors.isNotEmpty) {
    stderr.writeln('release ${args.first}:');
    for (final e in errors) {
      stderr.writeln('  $e');
    }
    exit(1);
  }
  stdout.writeln('release ${args.first}: versions and changelogs match');
}

/// What does not match [tag] in the repository at [root]; empty if all do.
List<String> releaseErrors(String tag, String root) {
  final errors = <String>[];
  if (!RegExp(r'^\d+\.\d+\.\d+(-[0-9A-Za-z.]+)?$').hasMatch(tag)) {
    errors.add('tag "$tag" is not a version (1.2.3 or 1.2.3-dev.4)');
  }
  String? read(String path) {
    final file = File('$root/$path');
    if (!file.existsSync()) {
      errors.add('$path is missing');
      return null;
    }
    return file.readAsStringSync();
  }

  void expect(String path, RegExp pattern, String what) {
    final text = read(path);
    if (text == null) return;
    final found = pattern.firstMatch(text)?[1];
    if (found != tag) errors.add('$path: $what is ${found ?? 'missing'}');
  }

  final version = RegExp(r'^version:\s*(\S+)', multiLine: true);
  final heading = RegExp(r'^## (\S+)', multiLine: true);
  expect('pubspec.yaml', version, 'version');
  expect('native/pubspec.yaml', version, 'version');
  expect(
    'lib/src/native/native_release.dart',
    RegExp("^const String wsVersion = '([^']*)';", multiLine: true),
    'wsVersion',
  );
  expect('CHANGELOG.md', heading, 'the latest entry');
  expect('native/CHANGELOG.md', heading, 'the latest entry');
  return errors;
}

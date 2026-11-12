@TestOn('vm')
library;

import 'dart:io';

import 'package:test/test.dart';

import '../../tool/native_checksums.dart' show releaseTargets;
import '../../tool/release_check.dart';

/// Runs a Dart script of tool/ from the repository root.
Future<ProcessResult> _dart(List<String> args) =>
    Process.run(Platform.resolvedExecutable, ['run', ...args]);

String _file(String target) => switch (target.split('-').first) {
  'windows' => 'ws_native.dll',
  'macos' || 'ios' => 'libws_native.dylib',
  _ => 'libws_native.so',
};

void main() {
  late Directory tmp;
  setUp(() => tmp = Directory.systemTemp.createTempSync('ws_release_'));
  tearDown(() => tmp.deleteSync(recursive: true));

  group('release_check', () {
    void write(String path, String text) => (File(
      '${tmp.path}/$path',
    )..createSync(recursive: true)).writeAsStringSync(text);

    void repo(String version, {String? nativeChangelog}) {
      write('pubspec.yaml', 'name: ws\nversion: $version\n');
      write('native/pubspec.yaml', 'name: ws_native\nversion: $version\n');
      write(
        'lib/src/native/native_release.dart',
        "const String wsVersion = '$version';\n",
      );
      write('CHANGELOG.md', '## $version\n\n- x\n\n## 1.0.0\n');
      write('native/CHANGELOG.md', '## ${nativeChangelog ?? version}\n');
      write('flutter/pubspec.yaml', 'name: ws_flutter\nversion: $version\n');
      write('flutter/CHANGELOG.md', '## $version\n');
    }

    test('this repository matches its own version', () {
      final version = RegExp(
        r'^version:\s*(\S+)',
        multiLine: true,
      ).firstMatch(File('pubspec.yaml').readAsStringSync())![1]!;
      expect(releaseErrors(version, '.'), isEmpty);
    });

    test('accepts a matching tree', () {
      repo('2.1.0-dev.3');
      expect(releaseErrors('2.1.0-dev.3', tmp.path), isEmpty);
    });

    test('names every mismatch', () {
      repo('2.1.0', nativeChangelog: '2.0.9');
      expect(releaseErrors('2.1.1', tmp.path), hasLength(7));
      expect(releaseErrors('2.1.0', tmp.path), [
        'native/CHANGELOG.md: the latest entry is 2.0.9',
      ]);
    });

    test('rejects a tag that is not a version', () {
      repo('2.1.0');
      expect(releaseErrors('v2.1.0', tmp.path), contains(contains('v2.1.0')));
    });

    test('reports missing files', () {
      expect(releaseErrors('2.1.0', tmp.path), hasLength(7));
    });
  });

  group('native_checksums --check', () {
    void libraries(Iterable<String> targets) {
      for (final t in targets) {
        File('${tmp.path}/$t/${_file(t)}')
          ..createSync(recursive: true)
          ..writeAsStringSync(t);
      }
    }

    Future<ProcessResult> check() =>
        _dart(['tool/native_checksums.dart', '--check', tmp.path]);

    test('passes for exactly the release targets', () async {
      libraries(releaseTargets);
      final write = await _dart(['tool/native_checksums.dart', tmp.path]);
      expect(write.exitCode, 0, reason: '${write.stderr}');
      final result = await check();
      expect(result.exitCode, 0, reason: '${result.stderr}');
      expect(result.stdout, contains('12 libraries verified'));
    });

    test('fails when a library changed after checksums.json', () async {
      libraries(releaseTargets);
      await _dart(['tool/native_checksums.dart', tmp.path]);
      File('${tmp.path}/linux-x64/libws_native.so').writeAsStringSync('x');
      final result = await check();
      expect(result.exitCode, 1);
      expect(result.stderr, contains('linux-x64/libws_native.so: hash'));
    });

    test('fails when a target is missing', () async {
      libraries(releaseTargets.skip(1));
      await _dart(['tool/native_checksums.dart', tmp.path]);
      final result = await check();
      expect(result.exitCode, 1);
      expect(result.stderr, contains('${releaseTargets.first}/'));
    });

    test('fails without checksums.json', () async {
      libraries(releaseTargets);
      final result = await check();
      expect(result.exitCode, 1);
      expect(result.stderr, contains('checksums.json is missing'));
    });
  });

  group('stage_native.sh', skip: Platform.isWindows ? 'bash script' : null, () {
    Future<ProcessResult> stage(List<String> args) =>
        Process.run('tool/stage_native.sh', ['${tmp.path}/out', ...args]);

    test('stages tracked files only, with the exact ws version', () async {
      final result = await stage(['2.0.0']);
      expect(result.exitCode, 0, reason: '${result.stderr}');
      final out = '${tmp.path}/out';
      final pubspec = File('$out/pubspec.yaml').readAsStringSync();
      expect(pubspec, contains("  ws: '>=2.0.0 <2.0.1'\n"));
      expect(pubspec, isNot(contains('publish_to')));
      expect(File('$out/rust/Cargo.toml').existsSync(), isTrue);
      expect(File('$out/hook/build.dart').existsSync(), isTrue);
      expect(Directory('$out/test').existsSync(), isFalse);
      expect(Directory('$out/rust/target').existsSync(), isFalse);
      expect(File('$out/rust/rust-toolchain.toml').existsSync(), isFalse);
      expect(File('$out/rust/deny.toml').existsSync(), isFalse);
      expect(File('$out/pubspec.lock').existsSync(), isFalse);
    });

    test('a pre-release allows only that pre-release', () async {
      final result = await stage(['2.1.0-dev.3']);
      expect(result.exitCode, 0, reason: '${result.stderr}');
      expect(
        File('${tmp.path}/out/pubspec.yaml').readAsStringSync(),
        contains("  ws: '>=2.1.0-dev.3 <2.1.0-dev.3.0'\n"),
      );
    });

    test('a release needs the third-party notices', () async {
      final missing = '${tmp.path}/THIRD_PARTY_LICENSES';
      final result = await stage(['2.0.0', '--release', missing]);
      expect(result.exitCode, 1);
      expect(result.stderr, contains('missing notices'));
    });

    test('refuses a directory inside the repository', () async {
      final result = await Process.run('tool/stage_native.sh', [
        'build/stage_test',
      ]);
      expect(result.exitCode, 2);
      expect(result.stderr, contains('outside the repository'));
      Directory('build/stage_test').deleteSync(recursive: true);
    });
  });

  group(
    'stage_flutter.sh',
    skip: Platform.isWindows ? 'bash script' : null,
    () {
      test('stages tracked files, ws from pub.dev', () async {
        final out = '${tmp.path}/out';
        final result = await Process.run('tool/stage_flutter.sh', [
          out,
          '2.1.0-dev.3',
        ]);
        expect(result.exitCode, 0, reason: '${result.stderr}');
        final pubspec = File('$out/pubspec.yaml').readAsStringSync();
        expect(pubspec, contains('  ws: ^2.1.0-dev.3\n'));
        expect(pubspec, isNot(contains('publish_to')));
        expect(pubspec, isNot(contains('path:')));
        expect(File('$out/lib/src/lifecycle.dart').existsSync(), isTrue);
        expect(File('$out/LICENSE').existsSync(), isTrue);
        expect(Directory('$out/.dart_tool').existsSync(), isFalse);
        expect(File('$out/pubspec.lock').existsSync(), isFalse);
      });

      test('refuses a directory inside the repository', () async {
        final result = await Process.run('tool/stage_flutter.sh', [
          'build/stage_test',
        ]);
        expect(result.exitCode, 2);
        Directory('build/stage_test').deleteSync(recursive: true);
      });
    },
  );
}

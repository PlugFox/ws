@TestOn('vm')
library;

import 'dart:convert';
import 'dart:io';

import 'package:test/test.dart';
import 'package:ws/src/native/install.dart';
import 'package:ws/src/native/native_release.dart';
import 'package:ws/src/native/sha256.dart';

String _sha(List<int> bytes) => (Sha256()..add(bytes)).close();

void main() {
  group('Sha256', () {
    test('FIPS 180-4 vectors', () {
      expect(
        _sha(const []),
        'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      );
      expect(
        _sha(utf8.encode('abc')),
        'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
      );
      expect(
        _sha(
          utf8.encode(
            'abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq',
          ),
        ),
        '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1',
      );
      expect(
        _sha(List.filled(1000000, 0x61)),
        'cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0',
      );
    });

    test('chunking does not change the digest', () {
      final data = List.generate(1000, (i) => i * 7 & 0xFF);
      for (final step in [1, 3, 63, 64, 65, 999]) {
        final digest = Sha256();
        for (var i = 0; i < data.length; i += step) {
          digest.add(data.sublist(i, (i + step).clamp(0, data.length)));
        }
        expect(digest.close(), _sha(data), reason: 'step $step');
      }
    });
  });

  group('installNativeLibrary', () {
    late Directory temp;
    late Directory mirror;
    late Directory out;
    final library = utf8.encode('not really a library');
    const target = 'linux-x64';

    setUp(() {
      temp = Directory.systemTemp.createTempSync('ws_install_');
      mirror = Directory('${temp.path}/mirror')..createSync();
      out = Directory('${temp.path}/out');
      File('${mirror.path}/$target-libws_native.so').writeAsBytesSync(library);
    });
    tearDown(() => temp.deleteSync(recursive: true));

    String pattern() => '${mirror.uri}{asset}';

    test('downloads and verifies', () async {
      final file = await installNativeLibrary(
        target: target,
        directory: out,
        urlPattern: pattern(),
        hashes: {'$target/libws_native.so': _sha(library)},
      );
      expect(file.path, endsWith('libws_native.so'));
      expect(file.readAsBytesSync(), library);
      expect(out.listSync(), hasLength(1), reason: 'no .part file left');
    });

    test('a checksum mismatch installs nothing', () async {
      await expectLater(
        installNativeLibrary(
          target: target,
          directory: out,
          urlPattern: pattern(),
          sha256: _sha(utf8.encode('something else')),
        ),
        throwsA(
          isA<NativeInstallException>().having(
            (e) => e.message,
            'message',
            contains('Checksum mismatch'),
          ),
        ),
      );
      expect(out.listSync(), isEmpty);
    });

    test('without a known hash nothing is downloaded', () async {
      await expectLater(
        installNativeLibrary(
          target: target,
          directory: out,
          urlPattern: pattern(),
          hashes: const {},
        ),
        throwsA(isA<NativeInstallException>()),
      );
      expect(out.existsSync(), isFalse);
    });

    test('a verified existing file is kept, a stale one replaced', () async {
      out.createSync();
      final existing = File('${out.path}/libws_native.so')
        ..writeAsBytesSync(library);
      final stamp = existing.lastModifiedSync();
      await installNativeLibrary(
        target: target,
        directory: out,
        urlPattern: '${temp.uri}missing/{asset}',
        sha256: _sha(library),
      );
      expect(existing.lastModifiedSync(), stamp);

      existing.writeAsStringSync('stale');
      await installNativeLibrary(
        target: target,
        directory: out,
        urlPattern: pattern(),
        sha256: _sha(library),
      );
      expect(existing.readAsBytesSync(), library);
    });

    test('HTTP errors are reported', () async {
      final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
      server.listen(
        (r) => r.response
          ..statusCode = 404
          ..close(),
      );
      addTearDown(server.close);
      await expectLater(
        installNativeLibrary(
          target: target,
          directory: out,
          urlPattern: 'http://127.0.0.1:${server.port}/{tag}/{asset}',
          sha256: _sha(library),
        ),
        throwsA(
          isA<NativeInstallException>().having(
            (e) => e.message,
            'message',
            contains('HTTP 404'),
          ),
        ),
      );
    });

    test('file names and host target', () {
      expect(nativeLibraryFileName('windows-x64'), 'ws_native.dll');
      expect(nativeLibraryFileName('macos-arm64'), 'libws_native.dylib');
      expect(nativeLibraryFileName('android-arm64'), 'libws_native.so');
      expect(hostNativeTarget(), matches(RegExp(r'^[a-z]+-[a-z0-9]+$')));
    });
  });

  test('wsVersion matches pubspec.yaml', () {
    // Windows checkouts may have CRLF line endings.
    final pubspec = File('pubspec.yaml').readAsLinesSync();
    expect(pubspec, contains('version: $wsVersion'));
  });

  group('parseInstallArgs', () {
    test('the README line: --key value', () {
      expect(parseInstallArgs(['--target', 'linux-x64', '--out', '/app/lib']), {
        'target': 'linux-x64',
        'out': '/app/lib',
      });
    });

    test('--key=value, --list', () {
      expect(parseInstallArgs(['--target=ios-arm64-sim', '--list']), {
        'target': 'ios-arm64-sim',
        'list': '',
      });
    });

    test('rejects unknown, missing and invalid values', () {
      for (final args in [
        ['linux-x64'],
        ['--bogus=1'],
        ['--target'],
        ['--target', '--out', 'x'],
        ['--target=../../etc'],
        ['--sha256=abc'],
      ]) {
        expect(
          () => parseInstallArgs(args),
          throwsFormatException,
          reason: '$args',
        );
      }
    });
  });
}

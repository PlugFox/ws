// `dart run ws:install`: downloads the native (Rust) library of this ws
// release for apps built with `dart compile exe` (DECISIONS.md D32).
import 'dart:async';
import 'dart:io';

import 'package:ws/src/native/install.dart';
import 'package:ws/src/native/native_release.dart';

const _usage =
    '''
Downloads the native library of ws $wsVersion and verifies its SHA-256.

Usage: dart run ws:install [options]

  --target=<os-arch>   linux-x64, linux-arm64, macos-arm64, windows-x64, ...
                       (default: this machine, ${'{host}'})
  --out=<directory>    where to put the library (default: current directory)
  --url=<pattern>      mirror; {tag} and {asset} are replaced
                       (default: $defaultNativeUrlPattern)
  --sha256=<hex>       expected hash, for a library not released with this
                       version (for example from a mirror build)
  --list               print the released targets and exit
  -h, --help

Then load it in the app:

  WsNativeLibrary.open('<out>/<file>').install();   // package:ws/native.dart
''';

Future<void> main(List<String> args) async {
  if (args.contains('-h') || args.contains('--help')) {
    stdout.write(_usage.replaceAll('{host}', hostNativeTarget()));
    return;
  }
  final Map<String, String> options;
  try {
    options = parseInstallArgs(args);
  } on FormatException catch (e) {
    return _fail(e.message);
  }
  if (options.containsKey('list')) {
    for (final key in nativeAssetHashes.keys) {
      stdout.writeln(key.split('/').first);
    }
    return;
  }
  final target = options['target'] ?? hostNativeTarget();
  try {
    final file = await installNativeLibrary(
      target: target,
      directory: Directory(options['out'] ?? '.'),
      urlPattern: options['url'] ?? defaultNativeUrlPattern,
      sha256: options['sha256'],
    );
    stdout
      ..writeln('Installed ${file.path} ($target, ws $wsVersion)')
      // The library contains Rust crates whose licences require their
      // notices next to binary copies (distribution.md F8).
      ..writeln(
        'Ship its third-party notices with it: '
        'https://github.com/PlugFox/ws/releases/download/'
        '$nativeReleaseTag/THIRD_PARTY_LICENSES',
      );
  } on NativeInstallException catch (e) {
    _fail(e.message);
  } on IOException catch (e) {
    _fail('$e');
  } on TimeoutException {
    _fail('the download stalled');
  }
}

void _fail(String message) {
  stderr.writeln('ws:install: $message');
  exitCode = 1;
}

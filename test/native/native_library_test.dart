@TestOn('vm')
library;

import 'dart:io';

import 'package:test/test.dart';
import 'package:ws/native.dart';

import '../support/test_server.dart';
import '../transport/conformance.dart';
import '../transport/server_suite.dart';

/// Path to a built `libws_native` (`cargo build --release` in
/// `native/rust`); the conformance group is skipped without it.
final String? _library = Platform.environment['WS_NATIVE_LIBRARY'];

void main() {
  test('a missing file is an ArgumentError', () {
    expect(
      () => WsNativeLibrary.open('/nonexistent/libws_native.so'),
      throwsArgumentError,
    );
  });

  test('another library is an ArgumentError naming the path', () {
    final system = Platform.isMacOS
        ? '/usr/lib/libSystem.B.dylib'
        : Platform.isWindows
        ? 'kernel32.dll'
        : 'libc.so.6';
    expect(
      () => WsNativeLibrary.open(system),
      throwsA(
        isArgumentError.having(
          (e) => e.message,
          'message',
          contains('not a ws_native library'),
        ),
      ),
    );
  });

  group(
    'loaded by path',
    () {
      late WsNativeLibrary library;
      setUpAll(() {
        library = WsNativeLibrary.open(_library!);
        expect(library.install(), isTrue);
      });
      transportConformance(() => library.transport());
      group(
        'native',
        () => serverConformance(() => library.transport()),
        // An inner `skip: false` would override the outer skip.
        skip: _library == null ? 'needs WS_NATIVE_LIBRARY' : TestServer.skip,
      );
    },
    skip: _library == null
        ? 'set WS_NATIVE_LIBRARY to a built libws_native to run'
        : false,
  );
}

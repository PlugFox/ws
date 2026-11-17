@TestOn('vm')
library;

import 'dart:io';

import 'package:test/test.dart';
import 'package:ws/src/native/native_library.dart';

// The ABI number is written by hand on both sides of the FFI boundary
// (docs/review/native.md finding 29): a change on one side only must fail
// here, not on a device.
void main() {
  test('Dart and Rust agree on the ABI version', () {
    final rust = File('native/rust/src/lib.rs').readAsStringSync();
    final match = RegExp(r'const ABI_VERSION: u32 = (\d+);').firstMatch(rust);
    expect(match, isNotNull, reason: 'ABI_VERSION not found in lib.rs');
    expect(int.parse(match!.group(1)!), nativeAbiVersion);
  });
}

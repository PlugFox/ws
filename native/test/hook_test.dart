// Pure parts of the build hook (docs/review/native.md finding 12).
import 'package:test/test.dart';

import '../hook/build.dart';

void main() {
  test('the NDK clang wrapper is a .cmd script on Windows', () {
    expect(
      ndkClang('aarch64-linux-android', 24, windows: false),
      'aarch64-linux-android24-clang',
    );
    expect(
      ndkClang('armv7a-linux-androideabi', 21, windows: true),
      'armv7a-linux-androideabi21-clang.cmd',
    );
  });

  test('cargo builds are kept apart per NDK level and deployment target', () {
    expect(targetKey('android', '21'), isNot(targetKey('android', '24')));
    expect(targetKey('ios', '13.0'), 'ios-13.0');
    expect(targetKey('default', null), 'default');
  });
}

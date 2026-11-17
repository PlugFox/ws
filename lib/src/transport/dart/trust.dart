import 'dart:convert' show base64, latin1;
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/client/options.dart';

/// The `SecurityContext` for [tls]; `null` for `dart:io`'s default.
///
/// On Android `dart:io` reads the same system directory Android does (the
/// Conscrypt APEX on Android 14+, else the system image) but ignores the
/// system CAs the user disabled and never reads user-installed CAs. When
/// either matters, both transports build the same store instead
/// (DECISIONS.md D45): the system CAs minus the disabled ones, plus the
/// user CAs with `trustUserCertificates`. Built once per setting: reading
/// the store costs about 30 ms on a phone.
io.SecurityContext? securityContext(WebSocketTls tls) {
  final roots = tls.trustedRootsPem;
  final user = io.Platform.isAndroid && tls.trustUserCertificates;
  final key = (roots, user);
  final cached = _contexts[key];
  if (cached != null) return cached;
  var certificates = const <Uint8List>[];
  if (io.Platform.isAndroid) {
    final dir = _androidUserDir();
    if (user || readCertificates('$dir/cacerts-removed').isNotEmpty) {
      certificates = androidRoots(
        systemDirs: _systemDirs,
        userDir: dir,
        user: user,
      );
    }
  }
  if (certificates.isEmpty && roots == null) return null;
  final context = io.SecurityContext(withTrustedRoots: certificates.isEmpty);
  if (certificates.isNotEmpty) {
    context.setTrustedCertificatesBytes(latin1.encode(toPem(certificates)));
  }
  if (roots != null) {
    context.setTrustedCertificatesBytes(Uint8List.fromList(roots.codeUnits));
  }
  // Apps use one or two settings; the bound stops unbounded growth.
  if (_contexts.length == 16) _contexts.remove(_contexts.keys.first);
  return _contexts[key] = context;
}

final _contexts = <(String?, bool), io.SecurityContext>{};

/// The updatable Conscrypt APEX (Android 14+) first, then the system image
/// (the order of `dart:io` and Conscrypt).
const _systemDirs = [
  '/apex/com.android.conscrypt/cacerts',
  '/system/etc/security/cacerts',
];

/// `/data/misc/user/<user id>`: per Android user (work profile, secondary
/// users). App processes may read it (group `everybody`).
String _androidUserDir() {
  var uid = 0;
  try {
    final line = io.File(
      '/proc/self/status',
    ).readAsLinesSync().firstWhere((l) => l.startsWith('Uid:'));
    uid = int.parse(line.split(RegExp(r'\s+'))[1]);
  } on Object {
    // Unknown: the owner (user 0).
  }
  return '/data/misc/user/${uid ~/ 100000}';
}

/// The Android trust store as Conscrypt's `TrustedCertificateStore` reads
/// it, as DER: the first non-empty of [systemDirs] minus
/// `<userDir>/cacerts-removed`, plus `<userDir>/cacerts-added` if [user].
/// Same rules as the Rust transport (`native/rust/src/tls.rs`).
List<Uint8List> androidRoots({
  required List<String> systemDirs,
  required String userDir,
  required bool user,
}) {
  final removed = readCertificates('$userDir/cacerts-removed');
  var system = const <Uint8List>[];
  for (final dir in systemDirs) {
    system = readCertificates(dir);
    if (system.isNotEmpty) break;
  }
  return [
    for (final c in system)
      if (!removed.any((r) => _same(r, c))) c,
    if (user) ...readCertificates('$userDir/cacerts-added'),
  ];
}

/// Every certificate in [dir], as DER. System files are PEM after a text
/// dump; the user store holds DER. Unreadable entries are skipped: a
/// missing directory means no such certificates.
List<Uint8List> readCertificates(String dir) {
  final out = <Uint8List>[];
  final List<io.FileSystemEntity> entries;
  try {
    entries = io.Directory(dir).listSync();
  } on io.FileSystemException {
    return out;
  }
  for (final entry in entries) {
    final Uint8List bytes;
    try {
      bytes = io.File(entry.path).readAsBytesSync();
    } on io.FileSystemException {
      continue;
    }
    if (bytes.isEmpty) continue;
    if (bytes.first == 0x30) {
      out.add(bytes);
    } else {
      out.addAll(_pemCertificates(latin1.decode(bytes)));
    }
  }
  return out;
}

const _begin = '-----BEGIN CERTIFICATE-----';
const _end = '-----END CERTIFICATE-----';

Iterable<Uint8List> _pemCertificates(String text) sync* {
  var from = 0;
  while (true) {
    final start = text.indexOf(_begin, from);
    if (start < 0) return;
    final end = text.indexOf(_end, start);
    if (end < 0) return;
    from = end + _end.length;
    try {
      yield base64.decode(
        text
            .substring(start + _begin.length, end)
            .replaceAll(RegExp(r'\s'), ''),
      );
    } on FormatException {
      // A broken entry is skipped, like the Rust side does.
    }
  }
}

/// [certificates] (DER) as one PEM text.
String toPem(List<Uint8List> certificates) {
  final out = StringBuffer();
  for (final der in certificates) {
    final text = base64.encode(der);
    out.writeln(_begin);
    for (var i = 0; i < text.length; i += 64) {
      out.writeln(
        text.substring(i, i + 64 > text.length ? text.length : i + 64),
      );
    }
    out.writeln(_end);
  }
  return out.toString();
}

bool _same(Uint8List a, Uint8List b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

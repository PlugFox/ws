import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/util/sha256.dart';

// X.509 pieces the pure-Dart transport needs (RFC 5280): host names for
// classifying a rejected certificate, the public key for pins. A minimal
// DER walk; anything unexpected is "not understood".

/// The TBSCertificate's elements of a DER certificate.
List<_Element> _tbs(Uint8List der) =>
    _Der(der, 0, der.length).children().first.children().first.children();

/// The SubjectPublicKeyInfo of a DER certificate, tag and length included
/// (what `openssl x509 -pubkey | openssl pkey -pubin -outform der`
/// prints); `null` when the encoding is not understood. The Rust
/// transport has the same walk (`native/rust/src/tls.rs`, `spki`).
Uint8List? subjectPublicKeyInfo(Uint8List der) {
  try {
    final tbs = _tbs(der);
    // version [0] is optional; then serialNumber, signature, issuer,
    // validity, subject, subjectPublicKeyInfo.
    final key = tbs[tbs.first.tag == 0xA0 ? 6 : 5];
    return key.tag == 0x30 ? key.encoded : null;
  } on Object {
    return null;
  }
}

/// Whether the SHA-256 of [der]'s SubjectPublicKeyInfo is one of [pins].
bool matchesPin(Uint8List der, List<Uint8List> pins) {
  final key = subjectPublicKeyInfo(der);
  if (key == null) return false;
  final digest = sha256(key);
  return pins.any((pin) => _sameBytes(pin, digest));
}

/// The DNS names and IP addresses of a certificate's subjectAltName
/// extension (RFC 5280 §4.2.1.6).
typedef AltNames = ({List<String> dns, List<Uint8List> ip});

/// Whether [host] is covered (RFC 6125: one leading `*` label at most).
extension AltNamesMatch on AltNames {
  /// Whether [host] (a name or an IP literal) is covered.
  bool matches(String host) {
    final address = io.InternetAddress.tryParse(host);
    if (address != null) {
      final raw = address.rawAddress;
      return ip.any((a) => _sameBytes(a, raw));
    }
    final name = _normalize(host);
    for (final pattern in dns.map(_normalize)) {
      if (pattern == name) return true;
      if (pattern.startsWith('*.')) {
        final dot = name.indexOf('.');
        if (dot > 0 && name.substring(dot) == pattern.substring(1)) {
          return true;
        }
      }
    }
    return false;
  }
}

String _normalize(String name) {
  final lower = name.toLowerCase();
  return lower.endsWith('.') ? lower.substring(0, lower.length - 1) : lower;
}

bool _sameBytes(List<int> a, List<int> b) {
  if (a.length != b.length) return false;
  for (var i = 0; i < a.length; i++) {
    if (a[i] != b[i]) return false;
  }
  return true;
}

/// Reads the subjectAltName extension from a DER certificate; `null` when
/// it is absent or the encoding is not understood.
AltNames? subjectAltNames(Uint8List der) {
  try {
    final tbs = _tbs(der);
    final extensions = tbs.where((e) => e.tag == 0xA3);
    if (extensions.isEmpty) return null;
    for (final extension in extensions.first.children().first.children()) {
      final parts = extension.children();
      if (!_sameBytes(parts.first.value, const [0x55, 0x1D, 0x11])) continue;
      final names = parts.last.value; // OCTET STRING
      final dns = <String>[];
      final ip = <Uint8List>[];
      for (final name in _Der(
        names,
        0,
        names.length,
      ).children().first.children()) {
        if (name.tag == 0x82) dns.add(String.fromCharCodes(name.value));
        if (name.tag == 0x87) ip.add(name.value);
      }
      return (dns: dns, ip: ip);
    }
    return null;
  } on Object {
    return null;
  }
}

/// A DER element list between [_start] and [_end] of [_bytes].
final class _Der {
  _Der(this._bytes, this._start, this._end);

  final Uint8List _bytes;
  final int _start;
  final int _end;

  /// The elements in this range.
  List<_Element> children() {
    final out = <_Element>[];
    var i = _start;
    while (i < _end) {
      final header = i;
      final tag = _bytes[i++];
      var length = _bytes[i++];
      if (length & 0x80 != 0) {
        final count = length & 0x7F;
        if (count == 0 || count > 4) throw const FormatException('DER length');
        length = 0;
        for (var k = 0; k < count; k++) {
          length = (length << 8) | _bytes[i++];
        }
      }
      if (i + length > _end) throw const FormatException('DER overrun');
      out.add(_Element(tag, _bytes, header, i, i + length));
      i += length;
    }
    return out;
  }
}

final class _Element {
  _Element(this.tag, this._bytes, this._header, this._start, this._end);

  final int tag;
  final Uint8List _bytes;
  final int _header;
  final int _start;
  final int _end;

  /// The content, without tag and length.
  Uint8List get value => Uint8List.sublistView(_bytes, _start, _end);

  /// The whole element: tag, length and content.
  Uint8List get encoded => Uint8List.sublistView(_bytes, _header, _end);

  List<_Element> children() => _Der(_bytes, _start, _end).children();
}

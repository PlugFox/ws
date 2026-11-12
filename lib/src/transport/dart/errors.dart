import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';

/// Classifies a failure while opening a connection (DECISIONS.md D42).
/// [viaProxy]: the socket was being opened to the proxy, so any socket
/// failure is a proxy failure. [tlsHost] is the server name TLS checked
/// (not the proxy).
WebSocketConnectionException connectFailure(
  Object error, {
  required String host,
  required bool viaProxy,
  String? tlsHost,
  io.X509Certificate? rejected,
}) {
  if (error is WebSocketConnectionException) return error;
  if (error is io.TlsException) {
    return WebSocketConnectionException(
      'TLS handshake failed',
      failure: WebSocketConnectionFailure.tls,
      tls: tlsFailure(error, rejected: rejected, host: tlsHost ?? host),
      cause: error,
    );
  }
  if (viaProxy) {
    return WebSocketConnectionException(
      'Could not connect to the proxy $host',
      failure: WebSocketConnectionFailure.proxy,
      cause: error,
    );
  }
  return WebSocketConnectionException(
    'Could not connect to $host',
    failure: error is io.SocketException
        ? socketFailure(error)
        : WebSocketConnectionFailure.other,
    cause: error,
  );
}

/// `ECONNREFUSED` on Linux/Android, Apple and Windows.
const _refused = {111, 61, 10061};

/// DNS, refused or unreachable, from a `dart:io` socket error.
WebSocketConnectionFailure socketFailure(io.SocketException error) {
  // dart:io reports resolver failures as "Failed host lookup: 'name'".
  if (error.message.startsWith('Failed host lookup')) {
    return WebSocketConnectionFailure.dns;
  }
  if (_refused.contains(error.osError?.errorCode)) {
    return WebSocketConnectionFailure.refused;
  }
  return WebSocketConnectionFailure.unreachable;
}

/// Classifies a BoringSSL error (`dart:io` puts its text in the message
/// or in `osError`). [rejected] is the certificate `onBadCertificate` saw:
/// on Apple platforms `dart:io` reports every verification failure as
/// "application verification failure", so the certificate's dates and
/// names are checked against [host] first.
WebSocketTlsFailure tlsFailure(
  io.TlsException error, {
  io.X509Certificate? rejected,
  String? host,
}) {
  if (rejected != null) {
    final now = DateTime.now();
    if (now.isAfter(rejected.endValidity)) return WebSocketTlsFailure.expired;
    if (now.isBefore(rejected.startValidity)) {
      return WebSocketTlsFailure.notYetValid;
    }
    final names = host == null ? null : subjectAltNames(rejected.der);
    if (names != null && !names.matches(host!)) {
      return WebSocketTlsFailure.hostname;
    }
  }
  final text = '${error.message} ${error.osError?.message ?? ''}'.toLowerCase();
  bool has(String s) => text.contains(s);
  if (has('hostname mismatch') || has('host name mismatch')) {
    return WebSocketTlsFailure.hostname;
  }
  if (has('certificate has expired') || has('certificate_expired')) {
    return WebSocketTlsFailure.expired;
  }
  if (has('not yet valid')) return WebSocketTlsFailure.notYetValid;
  if (has('revoked')) return WebSocketTlsFailure.revoked;
  if (has('certificate_verify_failed') ||
      has('unable to get local issuer') ||
      has('self signed') ||
      has('self-signed') ||
      has('unknown ca') ||
      has('bad certificate')) {
    return WebSocketTlsFailure.untrusted;
  }
  return WebSocketTlsFailure.handshake;
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
    final certificate = _Der(der, 0, der.length).children().first;
    final tbs = certificate.children().first;
    final extensions = tbs.children().where((e) => e.tag == 0xA3);
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
      out.add(_Element(tag, _bytes, i, i + length));
      i += length;
    }
    return out;
  }
}

final class _Element {
  _Element(this.tag, this._bytes, this._start, this._end);

  final int tag;
  final Uint8List _bytes;
  final int _start;
  final int _end;

  Uint8List get value => Uint8List.sublistView(_bytes, _start, _end);

  List<_Element> children() => _Der(_bytes, _start, _end).children();
}

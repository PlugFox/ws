import 'dart:io' as io;

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/transport/dart/certificate.dart';

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

/// `ECONNREFUSED` on Linux/Android and Apple; on Windows `WSAECONNREFUSED`
/// and `ERROR_CONNECTION_REFUSED` (what `dart:io` reports there).
const _refused = {111, 61, 10061, 1225};

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

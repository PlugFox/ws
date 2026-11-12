@TestOn('vm')
library;

import 'dart:io';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/transport/dart/errors.dart';
import 'package:ws/ws.dart';

void main() {
  test('socket errors: DNS, refused, unreachable', () {
    expect(
      socketFailure(const SocketException("Failed host lookup: 'x.invalid'")),
      WebSocketConnectionFailure.dns,
    );
    for (final code in [61, 111, 10061, 1225]) {
      expect(
        socketFailure(
          SocketException('Connection refused', osError: OSError('', code)),
        ),
        WebSocketConnectionFailure.refused,
      );
    }
    expect(
      socketFailure(
        const SocketException('No route', osError: OSError('', 65)),
      ),
      WebSocketConnectionFailure.unreachable,
    );
  });

  test('connect failures through a proxy are proxy failures', () {
    final e = connectFailure(
      const SocketException('Connection refused'),
      host: 'proxy',
      viaProxy: true,
    );
    expect(e.failure, WebSocketConnectionFailure.proxy);
  });

  test('BoringSSL texts', () {
    WebSocketTlsFailure classify(String text) => tlsFailure(
      HandshakeException('Handshake error in client', OSError(text)),
    );
    expect(
      classify('CERTIFICATE_VERIFY_FAILED: Hostname mismatch(handshake.cc:3)'),
      WebSocketTlsFailure.hostname,
    );
    expect(
      classify('CERTIFICATE_VERIFY_FAILED: certificate has expired'),
      WebSocketTlsFailure.expired,
    );
    expect(
      classify('CERTIFICATE_VERIFY_FAILED: certificate is not yet valid'),
      WebSocketTlsFailure.notYetValid,
    );
    expect(
      classify('CERTIFICATE_VERIFY_FAILED: certificate revoked'),
      WebSocketTlsFailure.revoked,
    );
    expect(
      classify(
        'CERTIFICATE_VERIFY_FAILED: unable to get local issuer certificate',
      ),
      WebSocketTlsFailure.untrusted,
    );
    expect(
      classify('WRONG_VERSION_NUMBER(tls_record.cc:231)'),
      WebSocketTlsFailure.handshake,
    );
  });

  group('subjectAltName', () {
    // DER of a SAN extension inside a minimal certificate skeleton:
    // Certificate { TBSCertificate { [3] { Extensions { Extension } } } }.
    Uint8List certificate(List<int> generalNames) {
      List<int> tlv(int tag, List<int> body) {
        assert(body.length < 128, 'short form only');
        return [tag, body.length, ...body];
      }

      final san = tlv(0x30, generalNames);
      final extension = tlv(0x30, [
        ...tlv(0x06, [0x55, 0x1D, 0x11]),
        ...tlv(0x04, san),
      ]);
      final tbs = tlv(0x30, [
        ...tlv(0x02, [1]),
        ...tlv(0xA3, tlv(0x30, extension)),
      ]);
      return Uint8List.fromList(tlv(0x30, tbs));
    }

    List<int> dns(String name) => [0x82, name.length, ...name.codeUnits];

    test('names and addresses are read and matched', () {
      final der = certificate([
        ...dns('example.com'),
        ...dns('*.example.org'),
        ...[0x87, 4, 127, 0, 0, 1],
      ]);
      final names = subjectAltNames(der)!;
      expect(names.dns, ['example.com', '*.example.org']);
      expect(names.matches('EXAMPLE.com.'), isTrue);
      expect(names.matches('a.example.org'), isTrue);
      expect(names.matches('a.b.example.org'), isFalse);
      expect(names.matches('example.org'), isFalse);
      expect(names.matches('127.0.0.1'), isTrue);
      expect(names.matches('127.0.0.2'), isFalse);
      expect(names.matches('other.com'), isFalse);
    });

    test('garbage and missing extensions give null', () {
      expect(subjectAltNames(Uint8List.fromList([0x30, 0x05, 1])), isNull);
      expect(
        subjectAltNames(Uint8List.fromList([0x30, 0x02, 0x30, 0x00])),
        isNull,
      );
    });
  });
}

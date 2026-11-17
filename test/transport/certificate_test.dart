@TestOn('vm')
library;

import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/client/tls_pins.dart';
import 'package:ws/src/transport/dart/certificate.dart';
import 'package:ws/src/transport/dart/trust.dart';
import 'package:ws/src/util/sha256.dart';

// Pins computed by OpenSSL 3.6:
// openssl x509 -pubkey -noout | openssl pkey -pubin -outform der |
//   openssl dgst -sha256 -binary | base64
const _rsa = '''
-----BEGIN CERTIFICATE-----
MIIDFTCCAf2gAwIBAgIUX13/c5JxJTk/Xl2YneHmOPLGGJEwDQYJKoZIhvcNAQEL
BQAwGjEYMBYGA1UEAwwPcGluIGZpeHR1cmUgcnNhMB4XDTI2MTExMjE4MjEyOFoX
DTM2MTEwOTE4MjEyOFowGjEYMBYGA1UEAwwPcGluIGZpeHR1cmUgcnNhMIIBIjAN
BgkqhkiG9w0BAQEFAAOCAQ8AMIIBCgKCAQEAtaYkyZqn0691UyiWGyVzEhUoqa47
8Kiy85/7q0+LvikST/KlefpPlkU9bIQS8KtbFMxJq1QGcvOtRxd1fph7CTSUG7RK
sq/UykX4kuOo+QHxi8XFC16X1zB3a5f7KhjeO5dVxJeNappD659vyntfRDcT0dck
QI43UCGaqqW84a4YBXmGICLr7tTk7eOej87y3dxaUA9AFqsdFhSJfuHIltzaPkOb
l/ydiVQp/4NFiEjEnzbFeTO9OpUAi/hDXy19NTC2O0k5el4bwhlR0UNkYXybtRpW
n7Ts1fp6Dy3jhcS6lFXuH7lJoNPfXz647A7kXaxM+chuzRxA3I4FhJWerwIDAQAB
o1MwUTAdBgNVHQ4EFgQU6DUm9V8paeI4eK0lbt4d0SDX0I0wHwYDVR0jBBgwFoAU
6DUm9V8paeI4eK0lbt4d0SDX0I0wDwYDVR0TAQH/BAUwAwEB/zANBgkqhkiG9w0B
AQsFAAOCAQEAHahsQ7PARo68FTcLtcTYPdzZwC+U4xSXDQa8V/rbgYfivJL12wDB
kR5p308/bMyuDRv1GmmwQi02Z8LiZmZ5pmJXJkBIbSPWIgroggOMKVXS5kMYik8x
9yKQo0t+kFitwgHJIAPaqD6EjQ9zbaA1MgcL5wDYyWX9ZUlFl+cIUjtt/XtbqW4+
xQ2lLNGq+ubTljSHbuePKgvmj478OqHoi4r1aoxN/xmzXZqWw7DRgX26cQEspCyT
4uGQ/a02tnRp3iYWMr1wI74YsXiuPkphyryPbZIjH6Qg7gy5AoknGV5LBPMtvbob
3VN+A9cf0cAr+1f3kSn1AveIkANeWQcfaA==
-----END CERTIFICATE-----
''';
const _rsaPin = 'p0dAcXTqjb1pOyqx9b1+gv64MBFcxSTFRp0WzCzKgG8=';

const _ec = '''
-----BEGIN CERTIFICATE-----
MIIBxDCCAUqgAwIBAgIUS4qhSpo//2srBawTAEovhx3gJ98wCgYIKoZIzj0EAwIw
GTEXMBUGA1UEAwwOcGluIGZpeHR1cmUgZWMwHhcNMjYxMTEyMTgyMTM4WhcNMzYx
MTA5MTgyMTM4WjAZMRcwFQYDVQQDDA5waW4gZml4dHVyZSBlYzB2MBAGByqGSM49
AgEGBSuBBAAiA2IABNTLfMSMPOh3gILIDKYXn92GsAlRxmkelcnQ9+kNhAGRVUEt
zx7RfXy7iTKNy7j4wBKcCFoTL86ve9fY3esG+80Gvg18KV5eE0KjOzRw01HQHKKg
NmbMyEJdyiQY0p7xTaNTMFEwHQYDVR0OBBYEFE7G1XIpwvlMyeGUo5u9kkogVgr5
MB8GA1UdIwQYMBaAFE7G1XIpwvlMyeGUo5u9kkogVgr5MA8GA1UdEwEB/wQFMAMB
Af8wCgYIKoZIzj0EAwIDaAAwZQIxAJH/+zyIFCkYp9r1V0kdc71Jc4llaWPz7U/I
KyDziNS2KmENJT20dqTs8GRXaT0kPQIwUhFfLxd2Ug4O3fASQigfD0EXFKi+I287
4abGmh/fjf6Wk15xwImPn78iODZrbR3e
-----END CERTIFICATE-----
''';
const _ecPin = 'if3U7Jiwl3/2MhGRAt/JaFJabO8K+rGnjmQ/ny4D3L4=';

Uint8List _der(String pem) => base64.decode(
  pem.replaceAll(RegExp('-----[A-Z ]+-----'), '').replaceAll(RegExp(r'\s'), ''),
);

String _hex(List<int> bytes) =>
    bytes.map((b) => b.toRadixString(16).padLeft(2, '0')).join();

/// One DER element with a minimal length.
List<int> _element(int tag, List<int> content) => [
  tag,
  if (content.length < 0x80)
    content.length
  else if (content.length < 0x100) ...[
    0x81,
    content.length,
  ] else ...[
    0x82,
    content.length >> 8,
    content.length & 0xFF,
  ],
  ...content,
];

/// The DER elements one after another in [content].
List<List<int>> _children(List<int> content) {
  final out = <List<int>>[];
  var i = 0;
  while (i < content.length) {
    final start = i;
    i++;
    var length = content[i++];
    if (length & 0x80 != 0) {
      final n = length & 0x7F;
      length = 0;
      for (var k = 0; k < n; k++) {
        length = (length << 8) | content[i++];
      }
    }
    i += length;
    out.add(content.sublist(start, i));
  }
  return out;
}

/// The content of a DER element, without tag and length.
List<int> _content(List<int> element) =>
    element.sublist(element[1] < 0x80 ? 2 : 2 + (element[1] & 0x7F));

/// [der] re-encoded as an X.509 v1 certificate: no `version` field (the
/// signature no longer matches; only the layout matters here).
Uint8List _withoutVersion(Uint8List der) {
  final certificate = _children(_content(der));
  final tbs = _children(_content(certificate.first));
  expect(tbs.first.first, 0xA0, reason: 'the fixture is v3');
  return Uint8List.fromList(
    _element(0x30, [
      ..._element(0x30, [for (final e in tbs.skip(1)) ...e]),
      ...certificate[1],
      ...certificate[2],
    ]),
  );
}

void main() {
  group('sha256 (FIPS 180-4 vectors, OpenSSL)', () {
    final cases = {
      '': 'e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855',
      'abc': 'ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad',
      'abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq':
          '248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1',
      // Padding edges: 55 bytes fit one block, 56 need two, 64 is one full.
      'x' * 55:
          'd5e285683cd4efc02d021a5c62014694958901005d6f71e89e0989fac77e4072',
      'x' * 56:
          '04c26261370ee7541549d16dee320c723e3fd14671e66a099afe0a377c16888e',
      'x' * 64:
          '7ce100971f64e7001e8fe5a51973ecdfe1ced42befe7ee8d5fd6219506b5393c',
    };
    for (final MapEntry(:key, :value) in cases.entries) {
      test('${key.length} bytes', () {
        expect(_hex(sha256(utf8.encode(key))), value);
      });
    }
    test('a million "a"', () {
      expect(
        _hex(sha256(Uint8List(1000000)..fillRange(0, 1000000, 0x61))),
        'cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0',
      );
    });
  });

  group('subjectPublicKeyInfo', () {
    test('matches OpenSSL for RSA and EC keys', () {
      expect(base64.encode(sha256(subjectPublicKeyInfo(_der(_rsa))!)), _rsaPin);
      expect(base64.encode(sha256(subjectPublicKeyInfo(_der(_ec))!)), _ecPin);
    });

    test('a v1 certificate (no version field)', () {
      final v1 = _withoutVersion(_der(_ec));
      expect(subjectPublicKeyInfo(v1), subjectPublicKeyInfo(_der(_ec)));
    });

    test('anything else is not understood', () {
      final der = _der(_rsa);
      expect(subjectPublicKeyInfo(Uint8List(0)), isNull);
      expect(subjectPublicKeyInfo(Uint8List.sublistView(der, 0, 100)), isNull);
      expect(subjectPublicKeyInfo(utf8.encode(_rsa)), isNull);
      // Six elements, but the sixth is not a SEQUENCE.
      final odd = _element(0x30, [
        ..._element(0x30, [
          for (var i = 0; i < 6; i++) ..._element(0x02, [i]),
        ]),
      ]);
      expect(subjectPublicKeyInfo(Uint8List.fromList(odd)), isNull);
    });

    test('matchesPin', () {
      final pins = pinDigests([_ecPin, 'sha256/$_rsaPin']);
      expect(matchesPin(_der(_rsa), pins), isTrue);
      expect(matchesPin(_der(_ec), pins.sublist(1)), isFalse);
      expect(matchesPin(Uint8List(3), pins), isFalse);
    });
  });

  group('pinDigests', () {
    test('base64, with or without sha256/, url-safe too', () {
      expect(pinDigests([_rsaPin]).single, base64.decode(_rsaPin));
      expect(pinDigests(['sha256/$_rsaPin']).single, base64.decode(_rsaPin));
      final urlSafe = _rsaPin.replaceAll('+', '-').replaceAll('/', '_');
      expect(pinDigests([urlSafe]).single, base64.decode(_rsaPin));
      expect(pinDigests(const []), isEmpty);
    });

    test('anything else names the pin', () {
      for (final bad in [
        '',
        'sha256/',
        'not base64!',
        base64.encode(Uint8List(31)),
        base64.encode(Uint8List(33)),
        'sha1/${base64.encode(Uint8List(20))}',
      ]) {
        expect(
          () => pinDigests([_rsaPin, bad]),
          throwsA(
            isA<FormatException>().having((e) => e.source, 'source', bad),
          ),
          reason: bad,
        );
      }
    });
  });

  group('Android trust store (D45)', () {
    late Directory dir;
    setUp(() => dir = Directory.systemTemp.createTempSync('ws_trust_'));
    tearDown(() => dir.deleteSync(recursive: true));

    void file(String path, List<int> bytes) => (File(
      '${dir.path}/$path',
    )..createSync(recursive: true)).writeAsBytesSync(bytes);

    /// System files: a text dump of the certificate, then PEM.
    List<int> systemFile(String pem) =>
        utf8.encode('Certificate:\n    Data: ...\n$pem');

    test('APEX first, disabled CAs removed, user CAs on request', () {
      file('apex/a.0', systemFile(_rsa));
      file('apex/b.0', systemFile(_ec));
      file('apex/junk.0', utf8.encode('not a certificate'));
      file(
        'apex/broken.0',
        utf8.encode(
          '-----BEGIN CERTIFICATE-----\n!!!\n-----END CERTIFICATE-----\n',
        ),
      );
      file('system/old.0', systemFile(_ec));
      // The user store holds DER.
      file('user/cacerts-removed/b.0', _der(_ec));
      file('user/cacerts-added/c.0', _der(_ec));

      List<Uint8List> roots(List<String> system, {required bool user}) =>
          androidRoots(
            systemDirs: [for (final d in system) '${dir.path}/$d'],
            userDir: '${dir.path}/user',
            user: user,
          );
      expect(roots(['apex', 'system'], user: false), [_der(_rsa)]);
      expect(roots(['apex', 'system'], user: true), [_der(_rsa), _der(_ec)]);
      expect(
        roots(['missing', 'system'], user: false),
        isEmpty,
        reason: 'the system copy of a disabled CA is removed too',
      );
      expect(roots(['missing'], user: true), [_der(_ec)]);
    });

    test('toPem round-trips through readCertificates', () {
      file('both.pem', utf8.encode(toPem([_der(_rsa), _der(_ec)])));
      expect(readCertificates(dir.path), [_der(_rsa), _der(_ec)]);
      expect(readCertificates('${dir.path}/missing'), isEmpty);
      // dart:io accepts it as a trust store.
      SecurityContext(withTrustedRoots: false).setTrustedCertificatesBytes(
        utf8.encode(toPem([_der(_rsa), _der(_ec)])),
      );
    });
  });
}

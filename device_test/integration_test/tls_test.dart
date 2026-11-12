// TLS trust on a real device, for both transports (DECISIONS.md D45).
// Run with tool/device_test.sh, which starts the server and passes:
//   WS_HOST, WSS_PORT  the wss endpoint (a leaf issued by a test CA)
//   CA_PEM             that CA (base64 of the PEM)
//   LEAF_PIN           the leaf's pin
//   USER_CA            1 if the CA is installed as a user CA (Android)
// ignore_for_file: implementation_imports
import 'dart:convert';
import 'dart:io';

import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';
import 'package:ws/src/transport/dart/dart_transport.dart';
import 'package:ws/src/transport/dart/trust.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';
import 'package:ws_native/ws_native.dart';

const _host = String.fromEnvironment('WS_HOST', defaultValue: 'localhost');
const _port = int.fromEnvironment('WSS_PORT', defaultValue: 9443);
const _caPemBase64 = String.fromEnvironment('CA_PEM');
const _leafPin = String.fromEnvironment('LEAF_PIN');
const _userCa = bool.fromEnvironment('USER_CA');
const _wrongPin = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=';

String get _caPem => utf8.decode(base64.decode(_caPemBase64));
String get _url => 'wss://$_host:$_port/echo';

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();

  // iOS answers "No route to host" until the local network permission is
  // granted (TN3179), which can take a moment after a fresh install.
  setUpAll(() async {
    for (var i = 0; ; i++) {
      try {
        final socket = await Socket.connect(_host, _port);
        socket.destroy();
        return;
      } on SocketException {
        if (i == 60) rethrow;
        await Future<void>.delayed(const Duration(milliseconds: 500));
      }
    }
  });

  test('configured', () {
    expect(_caPemBase64, isNotEmpty, reason: 'run tool/device_test.sh');
    expect(_leafPin, isNotEmpty);
  });

  final transports = <String, WebSocketTransport Function()>{
    'dart': () => const DartTransport(),
    'native': () => WsNative.library!.transport(),
  };

  test('the native library loads', () {
    expect(WsNative.install(), isTrue, reason: WsNative.unavailableReason);
  });

  for (final MapEntry(key: name, value: create) in transports.entries) {
    group(name, () {
      setUp(() => WebSocketTransports.use(create()));
      tearDown(() => WebSocketTransports.use(null));

      Future<Object?> connect(WebSocketTls tls) async {
        final client = WebSocketClient.text(
          options: WebSocketOptions(
            reconnect: const WebSocketReconnect.disabled(),
            vm: WebSocketVmOptions(tls: tls),
          ),
        );
        try {
          final watch = Stopwatch()..start();
          await client.connect(_url);
          final echo = client.messages.first;
          await client.push('hi');
          expect(await echo.timeout(const Duration(seconds: 5)), 'hi');
          // ignore: avoid_print
          print(
            '$name ${tls.trustUserCertificates ? 'user CAs ' : ''}'
            'connect+echo ${watch.elapsedMicroseconds / 1000} ms',
          );
          return null;
        } on WebSocketConnectionException catch (e) {
          return e.tls ?? e;
        } on WebSocketException catch (e) {
          return e;
        } finally {
          await client.close();
        }
      }

      test('pinned: the right key connects, another one does not', () async {
        expect(
          await connect(
            WebSocketTls(trustedRootsPem: _caPem, pins: [_leafPin]),
          ),
          isNull,
        );
        expect(
          await connect(
            WebSocketTls(trustedRootsPem: _caPem, pins: [_wrongPin]),
          ),
          WebSocketTlsFailure.pinMismatch,
        );
      });

      test(
        'a user CA: untrusted by default, trusted on request',
        skip: Platform.isAndroid && _userCa
            ? false
            : 'Android with the test CA installed as a user CA',
        () async {
          expect(
            await connect(const WebSocketTls()),
            WebSocketTlsFailure.untrusted,
          );
          expect(
            await connect(const WebSocketTls(trustUserCertificates: true)),
            isNull,
          );
          // Cached afterwards: a second connection reuses the store.
          expect(
            await connect(const WebSocketTls(trustUserCertificates: true)),
            isNull,
          );
        },
      );
    });
  }

  test('Android trust store', skip: !Platform.isAndroid, () {
    final user = '/data/misc/user/${_userId()}';
    final watch = Stopwatch()..start();
    final system = androidRoots(
      systemDirs: const [
        '/apex/com.android.conscrypt/cacerts',
        '/system/etc/security/cacerts',
      ],
      userDir: user,
      user: false,
    );
    final read = watch.elapsedMicroseconds;
    final all = androidRoots(
      systemDirs: const ['/apex/com.android.conscrypt/cacerts'],
      userDir: user,
      user: true,
    );
    final removed = readCertificates('$user/cacerts-removed');
    final added = readCertificates('$user/cacerts-added');
    watch.reset();
    SecurityContext(
      withTrustedRoots: false,
    ).setTrustedCertificatesBytes(utf8.encode(toPem(all)));
    final load = watch.elapsedMicroseconds;
    // ignore: avoid_print
    print(
      'store: ${system.length} system (after ${removed.length} '
      'disabled), ${added.length} user; read ${read / 1000} ms, '
      'SecurityContext ${load / 1000} ms',
    );
    expect(system, isNotEmpty);
    expect(all.length, system.length + added.length);
    if (_userCa) expect(added, isNotEmpty);
  });
}

int _userId() {
  final uid = File('/proc/self/status')
      .readAsLinesSync()
      .firstWhere((l) => l.startsWith('Uid:'))
      .split(RegExp(r'\s+'))[1];
  return int.parse(uid) ~/ 100000;
}

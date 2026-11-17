@TestOn('vm')
library;

import 'dart:io';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/src/transport/dart/keep_alive.dart';

int _get(RawSocket socket, int level, int option) {
  final value = socket.getRawOption(
    RawSocketOption(level, option, Uint8List(4)),
  );
  return ByteData.sublistView(value).getInt32(0, Endian.host);
}

void main() {
  test('keepalive options are set like in the native transport', () async {
    final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(server.close);
    final socket = await RawSocket.connect(
      InternetAddress.loopbackIPv4,
      server.port,
    );
    addTearDown(socket.close);
    expect(setKeepAlive(socket, const Duration(seconds: 30)), isTrue);
    final tcp = RawSocketOption.levelTcp;
    final level = RawSocketOption.levelSocket;
    if (Platform.isLinux || Platform.isAndroid) {
      expect(_get(socket, level, 9), isNot(0));
      expect(_get(socket, tcp, 4), 30);
      expect(_get(socket, tcp, 5), 10);
      expect(_get(socket, tcp, 6), 3);
      expect(_get(socket, tcp, 18), 60000);
    } else if (Platform.isMacOS || Platform.isIOS) {
      expect(_get(socket, level, 0x8), isNot(0));
      expect(_get(socket, tcp, 0x10), 30);
      expect(_get(socket, tcp, 0x101), 10);
      expect(_get(socket, tcp, 0x102), 3);
    } else if (Platform.isWindows) {
      expect(_get(socket, level, 0x8), isNot(0));
      expect(_get(socket, tcp, 3), 30);
      expect(_get(socket, tcp, 17), 10);
      expect(_get(socket, tcp, 16), 3);
    }
  });

  test('sub-second idle is rounded up to 1 s', () async {
    final server = await ServerSocket.bind(InternetAddress.loopbackIPv4, 0);
    addTearDown(server.close);
    final socket = await RawSocket.connect(
      InternetAddress.loopbackIPv4,
      server.port,
    );
    addTearDown(socket.close);
    expect(setKeepAlive(socket, Duration.zero), isTrue);
  });
}

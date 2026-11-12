// Hybrid echo server for browser tests (runs on the VM via spawnHybridUri).
import 'dart:io';

import 'package:stream_channel/stream_channel.dart';

Future<void> hybridMain(StreamChannel<Object?> channel) async {
  final server = await HttpServer.bind(InternetAddress.loopbackIPv4, 0);
  server.listen((request) async {
    if (request.uri.path == '/reject') {
      request.response.statusCode = 403;
      await request.response.close();
      return;
    }
    final socket = await WebSocketTransformer.upgrade(
      request,
      protocolSelector: (protocols) => protocols.last,
    );
    socket.listen((m) {
      if (m == 'close-4001') return socket.close(4001, 'bye').ignore();
      socket.add(m);
    });
  });
  channel.sink.add(server.port);
}

// ignore_for_file: avoid_print

import 'package:ws/ws.dart';

void main() async {
  const url = 'ws://localhost:1234';

  final client = WebSocketClient.connect(url);

  client.stream.listen((message) {
    print('Received message: $message');
  });

  client.add('Hello, server!');

  await Future<void>.delayed(const Duration(seconds: 10));
  client.close();
}

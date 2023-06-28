// ignore_for_file: avoid_print

import 'dart:async';

import 'package:ws/ws.dart';

void main() {
  const url = 'wss://echo.plugfox.dev:443/connect';

  final client = WebSocketClient.connect(url)
    ..stream.listen((message) {
      print('< $message');
    })
    ..add('Hello, ').ignore()
    ..add('world!').ignore();

  Timer(const Duration(seconds: 5), client.close);
}

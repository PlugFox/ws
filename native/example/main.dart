// Echo through the Rust transport. Run with `dart run example/main.dart`
// from native/ (the build hook bundles the library).
import 'dart:io';

import 'package:ws/ws.dart';
import 'package:ws_native/ws_native.dart';

Future<void> main() async {
  if (!WsNative.install()) {
    stderr.writeln('pure Dart transport: ${WsNative.unavailableReason}');
  }
  final client = WebSocketClient.text();
  client.messages.listen(print);
  await client.connect('wss://ws.postman-echo.com/raw');
  print('transport: ${client.metrics.transport}');
  await client.push('hello');
  await Future<void>.delayed(const Duration(seconds: 1));
  await client.close();
}

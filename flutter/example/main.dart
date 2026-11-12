// Ties a client to the Flutter app lifecycle: suspended in the background,
// checked and, if dead, replaced at once when the app returns.
import 'package:flutter/widgets.dart';
import 'package:ws/ws.dart';
import 'package:ws_flutter/ws_flutter.dart';

void main() {
  WidgetsFlutterBinding.ensureInitialized();
  final client = WebSocketClient.json();
  // Keep it for the app's lifetime; dispose() when the client goes.
  WebSocketLifecycle(client);
  client.connect('wss://example.com/feed').ignore();
  runApp(const SizedBox.shrink());
}

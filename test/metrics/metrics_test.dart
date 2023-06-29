import 'package:test/test.dart';
import 'package:ws/src/client/ws_client.dart';

void main() {
  group('metrics', () {
    test('from client', () async {
      final client =
          WebSocketClient.connect('wss://echo.plugfox.dev:443/connect');
      await client.add('Hello, world');
      await client.disconnect();
      await client.close();
      final metrics = client.metrics;
      expect(metrics.toString, returnsNormally);
      expect(metrics.hashCode, isA<int>());
      expect(metrics == metrics, isTrue);
      expect(metrics, equals(metrics));
    });
  });
}

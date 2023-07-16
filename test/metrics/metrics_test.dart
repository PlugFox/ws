import 'package:test/test.dart';
import 'package:ws/src/client/metrics.dart';
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

      // toJson, fromJson
      final json = metrics.toJson();
      expect(json, isA<Map<String, dynamic>>());
      final obj = WebSocketMetrics.fromJson(json);
      expect(obj.toString(), equals(metrics.toString()));
      expect(obj.disconnects, equals(metrics.disconnects));
      expect(obj.nextReconnectionAttempt?.millisecondsSinceEpoch,
          equals(metrics.nextReconnectionAttempt?.millisecondsSinceEpoch));
      expect(obj.currentReconnectAttempts,
          equals(metrics.currentReconnectAttempts));
      expect(obj.isReconnectionActive, equals(metrics.isReconnectionActive));
      expect(obj.lastDisconnect, equals(metrics.lastDisconnect));
      expect(obj.lastDisconnectTime?.millisecondsSinceEpoch,
          equals(metrics.lastDisconnectTime?.millisecondsSinceEpoch));
      expect(obj.lastSuccessfulConnectionTime?.millisecondsSinceEpoch,
          equals(metrics.lastSuccessfulConnectionTime?.millisecondsSinceEpoch));
      expect(obj.lastUrl, equals(metrics.lastUrl));
      expect(obj.receivedCount, equals(metrics.receivedCount));
      expect(obj.receivedSize, equals(metrics.receivedSize));
      expect(obj.readyState, equals(metrics.readyState));
      expect(obj.reconnects, equals(metrics.reconnects));
      expect(obj.timestamp.millisecondsSinceEpoch,
          equals(metrics.timestamp.millisecondsSinceEpoch));
      expect(obj.transferredCount, equals(metrics.transferredCount));
      expect(obj.transferredSize, equals(metrics.transferredSize));
    });
  });
}

import 'package:test/test.dart';
import 'package:ws/src/client/ws_client.dart';
import 'package:ws/src/manager/connection_manager.dart';

void main() => group('ConnectionManager', () {
      test('should not be a singleton', () {
        final client = WebSocketClient();
        expect(
          WebSocketConnectionManager(client),
          isNot(same(WebSocketConnectionManager(client))),
        );
        client.close();
      });

      test('backoffDelay', () {
        const fn = WebSocketConnectionManager.backoffDelay;
        const min = 500, max = 10000;
        for (var attempt = 0; attempt < 100; attempt++) {
          final delay = fn(attempt, min, max).inMilliseconds;
          expect(
            delay,
            allOf(greaterThanOrEqualTo(min), lessThanOrEqualTo(max)),
          );
        }
      });

      test('minDelay >= maxDelay', () {
        const fn = WebSocketConnectionManager.backoffDelay;
        const min = 10000, max = 500;
        for (var attempt = 0; attempt < 100; attempt++) {
          final delay = fn(attempt, min, max).inMilliseconds;
          expect(
            delay,
            equals(max),
          );
        }
      });
    });

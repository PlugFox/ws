import 'package:test/test.dart';
import 'package:ws/src/manager/connection_manager.dart';

void main() => group('ConnectionManager', () {
      test('should be a singleton', () {
        expect(
          WebSocketConnectionManager.instance,
          same(WebSocketConnectionManager.instance),
        );
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

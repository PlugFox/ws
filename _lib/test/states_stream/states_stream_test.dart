import 'package:test/test.dart';
import 'package:ws/interface.dart';

void main() {
  group('StateStream', () {
    test('whereType', () {
      expectLater(
        WebSocketStatesStream(
          Stream<WebSocketClientState>.value(
              const WebSocketClientState$Connecting(url: 'url')),
        ).whereType<WebSocketClientState$Connecting>(),
        emits(const WebSocketClientState$Connecting(url: 'url')),
      );
    });

    test('connecting', () {
      expectLater(
        WebSocketStatesStream(
          Stream<WebSocketClientState>.value(
              const WebSocketClientState$Connecting(url: 'url')),
        ).connecting,
        emits(const WebSocketClientState$Connecting(url: 'url')),
      );
    });

    test('open', () {
      expectLater(
        WebSocketStatesStream(
          Stream<WebSocketClientState>.value(
              const WebSocketClientState$Open(url: 'url')),
        ).open,
        emits(const WebSocketClientState$Open(url: 'url')),
      );
    });

    test('disconnecting', () {
      expectLater(
        WebSocketStatesStream(
          Stream<WebSocketClientState>.value(
              const WebSocketClientState$Disconnecting(
                  closeCode: 1000, closeReason: 'reason')),
        ).disconnecting,
        emits(const WebSocketClientState$Disconnecting(
            closeCode: 1000, closeReason: 'reason')),
      );
    });

    test('closed', () {
      expectLater(
        WebSocketStatesStream(
          Stream<WebSocketClientState>.value(const WebSocketClientState$Closed(
              closeCode: 1000, closeReason: 'reason')),
        ).closed,
        emits(const WebSocketClientState$Closed(
            closeCode: 1000, closeReason: 'reason')),
      );
    });
  });
}

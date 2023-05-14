import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WebSocketReadyState tests', () {
    test('WebSocketReadyState fromCode', () {
      expect(WebSocketReadyState.fromCode(0),
          equals(WebSocketReadyState.connecting));
      expect(WebSocketReadyState.fromCode(1), equals(WebSocketReadyState.open));
      expect(
          WebSocketReadyState.fromCode(2), equals(WebSocketReadyState.closing));
      expect(
          WebSocketReadyState.fromCode(3), equals(WebSocketReadyState.closed));
      expect(() => WebSocketReadyState.fromCode(4), throwsArgumentError);
    });

    test('WebSocketReadyState isConnecting, isOpen, isClosing, isClosed', () {
      expect(WebSocketReadyState.connecting.isConnecting, isTrue);
      expect(WebSocketReadyState.connecting.isOpen, isFalse);
      expect(WebSocketReadyState.connecting.isClosing, isFalse);
      expect(WebSocketReadyState.connecting.isClosed, isFalse);

      expect(WebSocketReadyState.open.isConnecting, isFalse);
      expect(WebSocketReadyState.open.isOpen, isTrue);
      expect(WebSocketReadyState.open.isClosing, isFalse);
      expect(WebSocketReadyState.open.isClosed, isFalse);

      expect(WebSocketReadyState.closing.isConnecting, isFalse);
      expect(WebSocketReadyState.closing.isOpen, isFalse);
      expect(WebSocketReadyState.closing.isClosing, isTrue);
      expect(WebSocketReadyState.closing.isClosed, isFalse);

      expect(WebSocketReadyState.closed.isConnecting, isFalse);
      expect(WebSocketReadyState.closed.isOpen, isFalse);
      expect(WebSocketReadyState.closed.isClosing, isFalse);
      expect(WebSocketReadyState.closed.isClosed, isTrue);
    });

    test('WebSocketReadyState toString', () {
      expect(WebSocketReadyState.connecting.toString(), equals('CONNECTING'));
      expect(WebSocketReadyState.open.toString(), equals('OPEN'));
      expect(WebSocketReadyState.closing.toString(), equals('CLOSING'));
      expect(WebSocketReadyState.closed.toString(), equals('CLOSED'));
    });
  });
}

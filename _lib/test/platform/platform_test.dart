import 'package:test/test.dart';
import 'package:ws/interface.dart';

void main() {
  group('WebSocketReadyState', () {
    test('fromCode() returns the correct state for each code', () {
      expect(WebSocketReadyState.fromCode(0), WebSocketReadyState.connecting);
      expect(WebSocketReadyState.fromCode(1), WebSocketReadyState.open);
      expect(
          WebSocketReadyState.fromCode(2), WebSocketReadyState.disconnecting);
      expect(WebSocketReadyState.fromCode(3), WebSocketReadyState.closed);
    });

    test('fromCode() throws ArgumentError for invalid code', () {
      expect(() => WebSocketReadyState.fromCode(4), throwsArgumentError);
    });
  });
}

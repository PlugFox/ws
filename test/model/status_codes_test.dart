import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WebSocketStatusCodes tests', () {
    test('valueOf returns correct enum for given code', () {
      expect(WebSocketStatusCodes.valueOf(1000),
          equals(WebSocketStatusCodes.normalClosure));
      expect(WebSocketStatusCodes.valueOf(1001),
          equals(WebSocketStatusCodes.goingAway));
      expect(WebSocketStatusCodes.valueOf(1002),
          equals(WebSocketStatusCodes.protocolError));
      expect(WebSocketStatusCodes.valueOf(1003),
          equals(WebSocketStatusCodes.unsupportedData));
      expect(WebSocketStatusCodes.valueOf(1004),
          equals(WebSocketStatusCodes.reserved));
    });

    test('valueOf returns null for invalid code', () {
      expect(WebSocketStatusCodes.valueOf(-1), isNull);
      expect(WebSocketStatusCodes.valueOf(999), isNull);
      expect(WebSocketStatusCodes.valueOf(1016), isNull);
    });

    test('compareTo returns correct result', () {
      expect(
          WebSocketStatusCodes.normalClosure
              .compareTo(WebSocketStatusCodes.normalClosure),
          equals(0));
      expect(
          WebSocketStatusCodes.normalClosure
              .compareTo(WebSocketStatusCodes.goingAway),
          lessThan(0));
      expect(
          WebSocketStatusCodes.goingAway
              .compareTo(WebSocketStatusCodes.normalClosure),
          greaterThan(0));
    });

    test('codename returns correct string', () {
      expect(WebSocketStatusCodes.normalClosure.codename,
          equals('NORMAL_CLOSURE'));
      expect(WebSocketStatusCodes.goingAway.codename, equals('GOING_AWAY'));
      expect(WebSocketStatusCodes.protocolError.codename,
          equals('PROTOCOL_ERROR'));
    });

    test('code returns correct int', () {
      expect(WebSocketStatusCodes.normalClosure.code, equals(1000));
      expect(WebSocketStatusCodes.goingAway.code, equals(1001));
      expect(WebSocketStatusCodes.protocolError.code, equals(1002));
    });
  });
}

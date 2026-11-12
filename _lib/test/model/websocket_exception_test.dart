// ignore_for_file: prefer_const_constructors

import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WSException tests', () {
    test('WSNotConnected code, message and toString', () {
      final exception = WSNotConnectedException();
      expect(exception, isA<WSException>());
      expect(exception.message, equals('WebSocket is not connected.'));
      expect(exception.toString(), equals('WebSocket is not connected.'));
      expect(exception.code, equals('ws_not_connected'));
      expect(exception.originalException, isNull);
    });

    test('WSClientClosed code, message and toString', () {
      final exception = WSClientClosedException();
      expect(exception, isA<WSException>());
      expect(exception.message, equals('WebSocket client is closed.'));
      expect(exception.toString(), equals('WebSocket client is closed.'));
      expect(exception.code, equals('ws_client_closed'));
      expect(exception.originalException, isNull);
    });

    test('WSSendException code, message and toString', () {
      final exception = WSSendException();
      expect(exception, isA<WSException>());
      expect(exception.message, equals('WebSocket send failed.'));
      expect(exception.toString(), equals('WebSocket send failed.'));
      expect(exception.code, equals('ws_send_exception'));
      expect(exception.originalException, isNull);
    });

    test('WSDisconnectException code, message and toString', () {
      final exception = WSDisconnectException();
      expect(exception, isA<WSException>());
      expect(exception.message,
          equals('WebSocket error occurred during disconnect.'));
      expect(exception.toString(),
          equals('WebSocket error occurred during disconnect.'));
      expect(exception.code, equals('ws_disconnect_exception'));
      expect(exception.originalException, isNull);
    });
  });
}

// ignore_for_file: prefer_const_constructors

import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WSException tests', () {
    test('WSNotConnected message and toString', () {
      final exception = WSNotConnected();
      expect(exception, isA<WSException>());
      expect(exception.message, equals('WebSocket is not connected.'));
      expect(exception.toString(), equals('WebSocket is not connected.'));
    });

    test('WSUnknownException message and toString', () {
      final exception = WSUnknownException();
      expect(exception, isA<WSException>());
      expect(exception.message, equals('An unknown WebSocket error occurred.'));
      expect(
          exception.toString(), equals('An unknown WebSocket error occurred.'));
    });

    test('WSSocketException message and toString', () {
      final exception = WSSocketException('Socket error.');
      expect(exception, isA<WSException>());
      expect(exception.message, equals('Socket error.'));
      expect(exception.toString(), equals('Socket error.'));
    });

    test('WSHttpException message and toString', () {
      final exception = WSHttpException('HTTP error.');
      expect(exception, isA<WSException>());
      expect(exception.message, equals('HTTP error.'));
      expect(exception.toString(), equals('HTTP error.'));
    });

    test('WSUnsupportedException message and toString', () {
      final exception = WSUnsupportedException('Unsupported operation.');
      expect(exception, isA<WSException>());
      expect(exception.message, equals('Unsupported operation.'));
      expect(exception.toString(), equals('Unsupported operation.'));
    });
  });
}

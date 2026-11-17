// ignore_for_file: prefer_const_constructors

import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WebSocketClientState tests', () {
    test(r'WebSocketClientState$Connecting properties', () {
      final state = WebSocketClientState$Connecting(url: 'ws://test');
      expect(state.readyState, equals(WebSocketReadyState.connecting));
      expect(state, equals(WebSocketClientState$Connecting(url: 'ws://test')));
      expect(state.toString(),
          equals('WebSocketClientState.connecting(ws://test)'));
    });

    test(r'WebSocketClientState$Open properties', () {
      final state = WebSocketClientState$Open(url: 'ws://test');
      expect(state.readyState, equals(WebSocketReadyState.open));
      expect(state.url, equals('ws://test'));
      expect(state.hashCode,
          equals(WebSocketReadyState.open.code ^ state.url.hashCode));
      expect(state, equals(WebSocketClientState$Open(url: 'ws://test')));
      expect(state.toString(), equals('WebSocketClientState.open(ws://test)'));
    });

    test(r'WebSocketClientState$Disconnecting properties', () {
      final state = WebSocketClientState$Disconnecting(
          closeCode: 1000, closeReason: 'test');
      expect(state.readyState, equals(WebSocketReadyState.disconnecting));
      expect(state.closeCode, equals(1000));
      expect(state.closeReason, equals('test'));
      expect(
          state,
          equals(WebSocketClientState$Disconnecting(
              closeCode: 1000, closeReason: 'test')));
      expect(
          state.toString(), equals('WebSocketClientState.disconnecting(test)'));
    });

    test(r'WebSocketClientState$Closed properties', () {
      final state =
          WebSocketClientState$Closed(closeCode: 1000, closeReason: 'test');
      expect(state.readyState, equals(WebSocketReadyState.closed));
      expect(state.closeCode, equals(1000));
      expect(state.closeReason, equals('test'));
      expect(
          state,
          equals(WebSocketClientState$Closed(
              closeCode: 1000, closeReason: 'test')));
      expect(state.toString(), equals('WebSocketClientState.closed(test)'));
    });

    test(r'WebSocketClientState$Connecting hashCode and ==', () {
      final state1 = WebSocketClientState$Connecting(url: 'ws://localhost');
      final state2 = WebSocketClientState$Connecting(url: 'ws://localhost');
      expect(state1.hashCode, equals(state2.hashCode));
      expect(state1, equals(state2));
    });

    test(r'WebSocketClientState$Open hashCode and ==', () {
      final state1 = WebSocketClientState$Open(url: 'ws://localhost');
      final state2 = WebSocketClientState$Open(url: 'ws://localhost');
      final state3 = WebSocketClientState$Open(url: 'ws://localhost:8080');
      expect(state1.hashCode, equals(state2.hashCode));
      expect(state1, equals(state2));
      expect(state1.hashCode, isNot(equals(state3.hashCode)));
      expect(state1, isNot(equals(state3)));
    });

    test(r'WebSocketClientState$Closing hashCode and ==', () {
      final state1 = WebSocketClientState$Disconnecting(
          closeCode: 1000, closeReason: 'Normal closure');
      final state2 = WebSocketClientState$Disconnecting(
          closeCode: 1000, closeReason: 'Normal closure');
      final state3 = WebSocketClientState$Disconnecting(
          closeCode: 1001, closeReason: 'Going away');
      expect(state1.hashCode, equals(state2.hashCode));
      expect(state1, equals(state2));
      expect(state1.hashCode, isNot(equals(state3.hashCode)));
      expect(state1, isNot(equals(state3)));
    });

    test(r'WebSocketClientState$Closed hashCode and ==', () {
      final state1 = WebSocketClientState$Closed(
          closeCode: 1000, closeReason: 'Normal closure');
      final state2 = WebSocketClientState$Closed(
          closeCode: 1000, closeReason: 'Normal closure');
      final state3 = WebSocketClientState$Closed(
          closeCode: 1001, closeReason: 'Going away');
      expect(state1.hashCode, equals(state2.hashCode));
      expect(state1, equals(state2));
      expect(state1.hashCode, isNot(equals(state3.hashCode)));
      expect(state1, isNot(equals(state3)));
    });
  });
}

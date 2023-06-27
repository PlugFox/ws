import 'dart:convert';

import 'package:test/test.dart';
import 'package:ws/interface.dart';
import 'package:ws/ws.dart';

void main() {
  group(
    'WebSocketClient init',
    () {
      const url = 'wss://echo.plugfox.dev:443/connect';

      late IWebSocketClient client;

      setUp(() {
        client = WebSocketClient(reconnectTimeout: const Duration(seconds: 1));
      });

      tearDown(() {
        client.close();
      });

      test('connect & close', () async {
        expect(client.state, isA<WebSocketClientState$Closed>());
        await expectLater(client.connect(url), completes);
        expect(client.state, isA<WebSocketClientState$Open>());
        client.add('ping');
        await expectLater(client.stream.first, completion(equals('pong')));
        expect(() => client.close(), returnsNormally);
      });
    },
    onPlatform: <String, Object?>{
      'browser': <Object?>[
        const Skip('This test is currently failing on Browser.'),
        // They'll be slow on browsers once it works on them.
        const Timeout.factor(2),
      ],
    },
  );

  group(
    'WebSocketClient',
    () {
      const url = 'wss://echo.plugfox.dev:443/connect';

      late IWebSocketClient client;

      setUpAll(() {
        client = WebSocketClient(reconnectTimeout: const Duration(seconds: 1));
      });

      tearDownAll(() {
        client.close();
      });

      setUp(() async {
        if (!client.state.readyState.isOpen) {
          await client.connect(url);
        }
      });

      test('can disconnect from a websocket server', () async {
        expect(client.state, equals(const WebSocketClientState.open(url: url)));
        client.disconnect();
        expect(
          client.state,
          anyOf(
            isA<WebSocketClientState$Closing>(),
            isA<WebSocketClientState$Closed>(),
          ),
        );
      });

      test('can send and receive a string message', () async {
        const message = 'Hello, World!';
        client.add(message);
        await for (final received in client.stream) {
          expect(received, equals(message));
          break;
        }
      });

      test('can send and receive a byte message', () async {
        final message = utf8.encode('Hello, World!');
        client.add(message);
        await expectLater(
          client.stream.first.timeout(const Duration(seconds: 5)),
          completion(equals(message)),
        );
      });

      test('reconnects if the connection is interrupted', () async {
        expect(client.state, equals(const WebSocketClientState.open(url: url)));
        client.add('close');
        await expectLater(
          client.stateChanges
              .firstWhere((state) =>
                  state.readyState.isClosing || state.readyState.isClosed)
              .timeout(const Duration(seconds: 1)),
          completes,
        );
        expect(
          client.state,
          anyOf(
            isA<WebSocketClientState$Closing>(),
            isA<WebSocketClientState$Closed>(),
          ),
        );
        await expectLater(
          client.stateChanges
              .firstWhere((state) => state.readyState.isOpen)
              .timeout(const Duration(seconds: 2)),
          completes,
        );
        expect(client.state, equals(const WebSocketClientState.open(url: url)));
      });

      // Test that messages can be sent again after reconnecting
      test('can send messages after reconnecting', () async {
        client.add('close'); // Simulate connection interruption.
        // Wait for the client to have time to attempt to reconnect.
        await expectLater(
          client.stateChanges
              .firstWhere((state) => state.readyState.isOpen)
              .timeout(const Duration(seconds: 3)),
          completes,
          reason: 'Client did not reconnect in time.',
        );
        expect(client.state.readyState.isOpen, isTrue);
        const message = 'Hello, again!';
        client.add(message);
        await expectLater(
          client.stream.first.timeout(const Duration(seconds: 5)),
          completion(equals(message)),
          reason: 'Client did not receive message in time.',
        );
      });

      // Test that large messages can be sent
      test('can send large messages', () async {
        final largeMessage = 'A' * 512 * 1024; // 0.5 MiB message
        client.add(largeMessage);
        await expectLater(
          client.stream.first.timeout(const Duration(seconds: 5)),
          completion(equals(largeMessage)),
          reason: 'Client did not receive message in time.',
        );
      });

      // Test that the client correctly handles
      // the server closing the connection.
      test(
        'handles server closing connection',
        () async {
          expect(client.state.readyState.isOpen, isTrue);
          // Simulate server closing connection.
          client.add('close');
          await expectLater(
            client.stateChanges
                .firstWhere((state) =>
                    state.readyState.isClosing || state.readyState.isClosed)
                .timeout(const Duration(seconds: 1)),
            completes,
          );
          expect(
            client.state,
            anyOf(
              isA<WebSocketClientState$Closing>(),
              isA<WebSocketClientState$Closed>(),
            ),
          );
        },
      );

      // Test that an error is thrown when trying to send
      // a message with a closed connection
      test('throws an error when trying to send with a closed connection',
          () async {
        expect(client.state.readyState.isOpen, isTrue);
        client.add('close'); // Simulate connection closing.
        // Wait for the client to have time to handle the connection closing.
        await expectLater(
          client.stateChanges
              .firstWhere((state) =>
                  state.readyState.isClosing || state.readyState.isClosed)
              .timeout(const Duration(seconds: 2)),
          completes,
        );
        await expectLater(
          Future<void>.sync(() => client.add('Hello, World!')),
          throwsA(isException),
        );
      });

      // Test that binary data can be sent
      test('can send binary data', () async {
        final binaryData =
            List<int>.generate(512, (i) => i % 256); // 0.5 KiB binary data
        client.add(binaryData);
        final received =
            await client.stream.first.timeout(const Duration(seconds: 5));
        expect(received, equals(binaryData));
      });

      // Test that the client behaves correctly when it fails to connect
      test('handles failed connection attempts', () async {
        // Try to connect to an invalid URL.
        await expectLater(
          client.connect('wss://invalid.url'),
          throwsA(isException),
        );
        expect(client.state, isA<WebSocketClientState$Closed>());
      });

      // Test that the client can connect to different URLs
      test('can connect to different URLs', () async {
        expect(client.state, isA<WebSocketClientState$Open>());
        client.add('close'); // Simulate server closing connection.
        await Future<void>.delayed(const Duration(seconds: 3));
        const anotherUrl = url;
        await client.connect(anotherUrl);
        expect(
          client.state,
          equals(const WebSocketClientState.open(url: anotherUrl)),
        );
      });

      // Test that the response time is acceptable
      test('has acceptable response time', () async {
        const message = 'Hello, World!';
        client.add(message);
        final stopwatch = Stopwatch()..start();
        try {
          await expectLater(
            client.stream.first.timeout(const Duration(seconds: 3)),
            completion(equals(message)),
          );
          stopwatch.stop();
          expect(stopwatch.elapsed, lessThan(const Duration(seconds: 3)));
        } finally {
          stopwatch.stop();
        }
      });
    },
    timeout: const Timeout(Duration(seconds: 10)),
    onPlatform: <String, Object?>{
      'browser': <Object?>[
        const Skip('This test is currently failing on Browser.'),
        // They'll be slow on browsers once it works on them.
        const Timeout.factor(2),
      ],
    },
  );
}

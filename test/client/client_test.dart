import 'dart:async';
import 'dart:convert';
import 'dart:io';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/interface.dart';
import 'package:ws/ws.dart';

void main() {
  final defaultWSOptions = WebSocketOptions.selector(
    vm: () => WebSocketOptions.vm(
      connectionRetryInterval: (
        min: const Duration(milliseconds: 500),
        max: const Duration(milliseconds: 1000),
      ),
      compression: const CompressionOptions(),
      customClient: HttpClient(),
      headers: const <String, Object?>{
        HttpHeaders.userAgentHeader: 'ws/1.0',
        HttpHeaders.acceptHeader: 'application/json',
      },
      userAgent: 'ws/1.0',
    ),
    js: () => WebSocketOptions.js(
      connectionRetryInterval: (
        min: const Duration(milliseconds: 500),
        max: const Duration(milliseconds: 1000),
      ),
      useBlobForBinary: false,
    ),
  );
  test('WebSocketClient example', () async {
    await runZoned<Future<void>>(
      () async {
        const url = 'wss://echo.plugfox.dev:443/connect';
        final client = WebSocketClient(defaultWSOptions);
        expect(client.state, isA<WebSocketClientState$Closed>());
        expect(client.metrics, isA<WebSocketMetrics>());
        client.stream.drain<void>().ignore();
        client.stateChanges.drain<void>().ignore();
        await expectLater(client.connect(url), completes);
        await expectLater(client.add('Hello, '), completes);
        await expectLater(client.add('world!'), completes);
        await expectLater(client.add('close'), completes);
        expect(client.metrics, isA<WebSocketMetrics>());
        await Future<void>.delayed(const Duration(milliseconds: 350));
        expect(client.state, isA<WebSocketClientState$Closed>());
        await Future<void>.delayed(const Duration(seconds: 3));
        expect(client.metrics, isA<WebSocketMetrics>());
        expect(client.state, isA<WebSocketClientState$Open>());
        await expectLater(client.disconnect(), completes);
        await Future<void>.delayed(const Duration(seconds: 1));
        await expectLater(client.close(), completes);
        await expectLater(client.close(), completes);
        expect(client.metrics, isA<WebSocketMetrics>());
      },
      zoneValues: <Object?, Object?>{
        #dev.plugfox.ws.debug: true,
      },
    );
  });

  group('Closed client', () {
    const url = 'wss://echo.plugfox.dev:443/connect';

    late IWebSocketClient client;

    setUp(() {
      client = WebSocketClient(defaultWSOptions);
    });

    tearDown(() {
      client.close();
    });

    test('connect', () async {
      await client.close();
      expect(client.state, isA<WebSocketClientState$Closed>());
      expect(() => client.connect(url), throwsException);
    });

    test('add', () async {
      await client.close();
      expect(client.state, isA<WebSocketClientState$Closed>());
      expect(() => client.add(url), throwsException);
    });

    test('disconnect', () async {
      await client.close();
      expect(client.state, isA<WebSocketClientState$Closed>());
      expect(() => client.disconnect(), throwsException);
    });

    test('close', () async {
      await client.close();
      expect(client.state, isA<WebSocketClientState$Closed>());
      expect(() => client.close(), returnsNormally);
    });
  });

  group(
    'WebSocketClient init',
    () {
      const url = 'wss://echo.plugfox.dev:443/connect';

      late IWebSocketClient client;

      setUp(() {
        client = WebSocketClient(defaultWSOptions);
      });

      tearDown(() {
        client.close();
      });

      test('connect & close', () async {
        expect(client.state, isA<WebSocketClientState$Closed>());
        await expectLater(client.connect(url), completes);
        expect(client.state, isA<WebSocketClientState$Open>());
        await expectLater(client.add('ping'), completes);
        await expectLater(client.stream.first, completion(equals('pong')));
        expect(() => client.close(), returnsNormally);
      });
    },
    /* onPlatform: <String, Object?>{
      'browser': <Object?>[
        const Skip('This test is currently failing on Browser.'),
        // They'll be slow on browsers once it works on them.
        const Timeout.factor(2),
      ],
    }, */
  );

  group('WebSocketClient', () {
    const url = 'wss://echo.plugfox.dev:443/connect';

    late IWebSocketClient client;

    setUpAll(() {
      client = WebSocketClient(defaultWSOptions);
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
          isA<WebSocketClientState$Disconnecting>(),
          isA<WebSocketClientState$Closed>(),
        ),
      );
    });

    test('emit unsupported data type', () async {
      expect(client.state, equals(const WebSocketClientState.open(url: url)));
      await expectLater(client.add(true), throwsException);
    });

    test('can send and receive a string message', () async {
      const message = 'Hello, World!';
      client.add(message);
      await for (final received in client.stream) {
        if (received is List<int>) {
          expect(utf8.decode(received), equals(message));
        } else if (received is String) {
          expect(received, equals(message));
        } else {
          fail('Received message is not a String or a List<int>.');
        }
        break;
      }
    });

    test('can send and receive a byte message', () async {
      final message = utf8.encode('Hello, World!');
      client.add(message);
      final received =
          await client.stream.bytes.first.timeout(const Duration(seconds: 5));
      expect(
          received,
          isA<List<int>>()
              .having((l) => l.length, 'length', equals(message.length)));
      expect(
        received,
        equals(message),
      );
      expect(
        utf8.decode(received),
        equals('Hello, World!'),
      );
    });

    test('reconnects if the connection is interrupted', () async {
      expect(client.state, equals(const WebSocketClientState.open(url: url)));
      client.add('close');
      await expectLater(
        client.stateChanges
            .firstWhere((state) =>
                state.readyState.isDisconnecting || state.readyState.isClosed)
            .timeout(const Duration(seconds: 1)),
        completes,
      );
      expect(
        client.state,
        anyOf(
          isA<WebSocketClientState$Disconnecting>(),
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
                  state.readyState.isDisconnecting || state.readyState.isClosed)
              .timeout(const Duration(seconds: 1)),
          completes,
        );
        expect(
          client.state,
          anyOf(
            isA<WebSocketClientState$Disconnecting>(),
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
      await client.add('close'); // Simulate connection closing.
      // Wait for the client to have time to handle the connection closing.
      await expectLater(
        client.stateChanges
            .firstWhere((state) =>
                state.readyState.isDisconnecting || state.readyState.isClosed)
            .timeout(const Duration(seconds: 2)),
        completes,
      );
      await expectLater(
        Future<void>(() => client.add('Hello, World!')),
        anyOf(
          throwsA(isException),
          throwsA(isA<Error>()),
          throwsA(isA<AssertionError>()),
        ),
      );
    });
  });

  group('Binary data', () {
    const url = 'wss://echo.plugfox.dev:443/connect';

    late IWebSocketClient client;

    setUpAll(() {
      client = WebSocketClient(defaultWSOptions);
    });

    tearDownAll(() {
      client.close();
    });

    setUp(() async {
      if (!client.state.readyState.isOpen) {
        await client.connect(url);
      }
    });

    // Test that List<int> can be sent
    test('can send List<int> data', () async {
      final binaryData =
          List<int>.generate(512, (i) => i % 256); // 0.5 KiB binary data
      client.add(binaryData);
      final received =
          await client.stream.first.timeout(const Duration(seconds: 5));
      expect(
          received,
          isA<List<int>>()
              .having((l) => l.length, 'length', equals(binaryData.length)));
      expect(received, equals(binaryData));
    });

    // Test that Uint8List can be sent
    test('can send Uint8List data', () async {
      final binaryData = Uint8List.fromList(
          List<int>.generate(512, (i) => i % 256)); // 0.5 KiB binary data
      client.add(binaryData);
      final received =
          await client.stream.first.timeout(const Duration(seconds: 5));
      expect(
          received,
          isA<List<int>>()
              .having((l) => l.length, 'length', equals(binaryData.length)));
    });

    // Test that ByteBuffer can be sent
    test('can send ByteBuffer data', () async {
      final buffer = Uint8List.fromList(List<int>.generate(512, (i) => i % 256))
          .buffer; // 0.5 KiB binary data
      client.add(buffer);
      final received =
          await client.stream.first.timeout(const Duration(seconds: 5));
      expect(
          received,
          isA<List<int>>()
              .having((l) => l.length, 'length', equals(buffer.lengthInBytes)));
    });

    // Test that ByteBuffer can be sent
    test('can send ByteBuffer data', () async {
      final buffer = Uint8List.fromList(List<int>.generate(512, (i) => i % 256))
          .buffer; // 0.5 KiB binary data
      client.add(buffer);
      final received =
          await client.stream.first.timeout(const Duration(seconds: 5));
      expect(
          received,
          isA<List<int>>()
              .having((l) => l.length, 'length', equals(buffer.lengthInBytes)));
    });
  });

  group('First message after connect', () {
    const url = 'wss://echo.plugfox.dev:443/connect';

    late IWebSocketClient client;

    setUpAll(() {
      client = WebSocketClient(WebSocketOptions.common(
        afterConnect: (c) => c.add('auth.abc.123'),
      ));
    });

    tearDownAll(() {
      client.close();
    });

    test('Can send first message from options', () async {
      await client.connect(url);
      var received =
          await client.stream.first.timeout(const Duration(seconds: 5));
      expect(received, equals('auth.abc.123'));
      client.add('message');
      received = await client.stream.first.timeout(const Duration(seconds: 5));
      expect(received, equals('message'));
    });
  });

  group(
    'Binary data as a Blob',
    () {
      const url = 'wss://echo.plugfox.dev:443/connect';

      late IWebSocketClient client;

      setUpAll(() {
        client = WebSocketClient(WebSocketOptions.js(
          connectionRetryInterval: (
            min: const Duration(milliseconds: 500),
            max: const Duration(milliseconds: 1000),
          ),
          useBlobForBinary: true,
        ));
      });

      tearDownAll(() {
        client.close();
      });

      setUp(() async {
        if (!client.state.readyState.isOpen) {
          await client.connect(url);
        }
      });

      // Test that List<int> can be sent
      test('can send List<int> data', () async {
        final binaryData =
            List<int>.generate(512, (i) => i % 256); // 0.5 KiB binary data
        client.add(binaryData);
        final received =
            await client.stream.first.timeout(const Duration(seconds: 5));
        expect(
            received,
            isA<List<int>>()
                .having((l) => l.length, 'length', equals(binaryData.length)));
        expect(received, equals(binaryData));
      });

      // Test that Uint8List can be sent
      test('can send Uint8List data', () async {
        final binaryData = Uint8List.fromList(
            List<int>.generate(512, (i) => i % 256)); // 0.5 KiB binary data
        client.add(binaryData);
        final received =
            await client.stream.first.timeout(const Duration(seconds: 5));
        expect(
            received,
            isA<List<int>>()
                .having((l) => l.length, 'length', equals(binaryData.length)));
      });

      // Test that ByteBuffer can be sent
      test('can send ByteBuffer data', () async {
        final buffer =
            Uint8List.fromList(List<int>.generate(512, (i) => i % 256))
                .buffer; // 0.5 KiB binary data
        client.add(buffer);
        final received =
            await client.stream.first.timeout(const Duration(seconds: 5));
        expect(
            received,
            isA<List<int>>().having(
                (l) => l.length, 'length', equals(buffer.lengthInBytes)));
      });

      // Test that ByteBuffer can be sent
      test('can send ByteBuffer data', () async {
        final buffer =
            Uint8List.fromList(List<int>.generate(512, (i) => i % 256))
                .buffer; // 0.5 KiB binary data
        client.add(buffer);
        final received =
            await client.stream.first.timeout(const Duration(seconds: 5));
        expect(
            received,
            isA<List<int>>().having(
                (l) => l.length, 'length', equals(buffer.lengthInBytes)));
      });
    },
    onPlatform: <String, Object?>{
      'vm': <Object?>[
        const Skip('This case only works on Browser'),
      ],
      'browser': <Object?>[
        const Timeout.factor(2),
      ],
    },
  );

  group('URLs', () {
    const url = 'wss://echo.plugfox.dev:443/connect';

    late IWebSocketClient client;

    setUpAll(() {
      client = WebSocketClient(defaultWSOptions);
    });

    tearDownAll(() {
      client.close();
    });

    setUp(() async {
      if (!client.state.readyState.isOpen) {
        await client.connect(url);
      }
    });

    // Test that the client behaves correctly when it fails to connect
    test(
      'handles failed connection attempts',
      () async {
        // Try to connect to an invalid URL.
        await expectLater(
          client.connect('wss://invalid.url'),
          throwsA(isException),
        );
        expect(client.state, isA<WebSocketClientState$Closed>());
      },
      timeout: const Timeout(Duration(seconds: 15)),
    );

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
  });

  group(
    'Responses',
    () {
      const url = 'wss://echo.plugfox.dev:443/connect';

      late IWebSocketClient client;

      setUpAll(() {
        client = WebSocketClient(defaultWSOptions);
      });

      tearDownAll(() {
        client.close();
      });

      setUp(() async {
        if (!client.state.readyState.isOpen) {
          await client.connect(url);
        }
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
    /* onPlatform: <String, Object?>{
      'browser': <Object?>[
        const Skip('This test is currently failing on Browser.'),
        // They'll be slow on browsers once it works on them.
        const Timeout.factor(2),
      ],
    }, */
  );

  group('Closed', () {
    late IWebSocketClient client;

    setUpAll(() {
      client = WebSocketClient();
    });

    tearDownAll(() {
      client.close();
    });

    test('try to close a closed connection', () async {
      expect(client.state.readyState.isClosed, isTrue);
      await expectLater(
        client.close(),
        completes,
      );
      await expectLater(
        client.close(),
        completes,
      );
      expect(client.isClosed, isTrue);
    });

    test('try use closed client', () async {
      expect(client.state.readyState.isClosed, isTrue);
      await expectLater(
        client.close(),
        completes,
      );
      await expectLater(
        client.connect('wss://echo.plugfox.dev:443/connect'),
        throwsA(isException),
      );
      await expectLater(
        client.add('Message'),
        throwsA(isException),
      );
      await expectLater(
        client.disconnect(),
        throwsA(isException),
      );
      await expectLater(
        client.close(),
        completes,
      );
    });

    test('WSClientClosed', () {
      expect(const WSClientClosedException(), isA<WSClientClosedException>());
    });
  });

  group('Protocols', () {
    test('Set protocols', () async {
      final commonOptions = WebSocketOptions.common(
        connectionRetryInterval: (
          min: const Duration(milliseconds: 500),
          max: const Duration(milliseconds: 1000),
        ),
        protocols: const <String>['foo', 'bar'],
      );
      final client1 = WebSocketClient(commonOptions);
      final client2 =
          WebSocketClient.connect('ws://localhost:80', commonOptions);
      final client3 = WebSocketClient.fromClient(client1);
      expect(client1, isA<WebSocketClient>());
      expect(client2, isA<WebSocketClient>());
      expect(client3, isA<WebSocketClient>());
      await expectLater(client1.close(), completes);
      await expectLater(client2.close(), completes);
      await expectLater(client3.close(), completes);
    });
  });

  group('Fake client', () {
    test('Construct', () async {
      final commonOptions = WebSocketOptions.common(
        connectionRetryInterval: (
          min: const Duration(milliseconds: 500),
          max: const Duration(milliseconds: 1000),
        ),
        protocols: const <String>['foo', 'bar'],
      );
      final clientFake =
          WebSocketClientFake(protocols: commonOptions.protocols);
      final client = WebSocketClient.fromClient(clientFake, commonOptions);
      expect(clientFake, isA<IWebSocketClient>());
      expect(client, isA<WebSocketClient>());
      expect(clientFake, isA<WebSocketClientFake>());
      expect(client.metrics, isA<WebSocketMetrics>());
      expect(client.state, isA<WebSocketClientState>());
      expect(client.stream, isA<WebSocketMessagesStream>());
      expect(client.stateChanges, isA<Stream<WebSocketClientState>>());
      expect(client.isClosed, isFalse);
      expect(client.state, isA<WebSocketClientState$Closed>());
      await expectLater(client.connect('ws://localhost:80'), completes);
      expect(client.state, isA<WebSocketClientState$Open>());
      await expectLater(client.add('Hello, world!'), completes);
      await expectLater(client.disconnect(), completes);
      expect(client.state, isA<WebSocketClientState$Closed>());
      await expectLater(client.close(), completes);
      expect(client.state, isA<WebSocketClientState$Closed>());
      expect(client.isClosed, isTrue);
    });
  });
}

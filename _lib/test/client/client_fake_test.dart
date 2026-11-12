import 'package:test/test.dart';
import 'package:ws/interface.dart';

void main() => group('Fake client', () {
      late WebSocketClientFake client;

      setUp(() {
        client = WebSocketClientFake(protocols: {'fake'});
      });

      tearDown(() {
        client.close();
      });

      test('Fake client', () async {
        expect(client, isA<IWebSocketClient>());
        expect(client, isA<WebSocketClientFake>());
        expect(client.isClosed, isFalse);
        expect(client.protocols, equals({'fake'}));
        expect(client.state.readyState.isOpen, isFalse);
        await expectLater(client.connect('url'), completes);
        expect(client.state.readyState.isOpen, isTrue);
        expect(() => client.loopBack('Hello, world'), returnsNormally);
        await Future<void>.delayed(const Duration(milliseconds: 25));
        expect(() => client.close(), returnsNormally);
        expect(client.state.readyState.isOpen, isFalse);
      });
    });

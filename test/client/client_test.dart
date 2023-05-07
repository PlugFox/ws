import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  group('WebSocketClient', () {
    const String url = 'ws://localhost:9090/connect';

    /* test('connect throws ArgumentError for invalid url', () {
      final client = WebSocketClient();
      expect(() => client.connect(''), throwsArgumentError);
    }); */

    test('connect', () async {
      final client = WebSocketClient();
      final connection = expectLater(client.connect(url), completes);
      await connection;
      client.add('ping');
      await expectLater(client.stream.first, completion(equals('pong')));
      expect(() => client.disconnect(), returnsNormally);
    });
  });
}

import 'dart:convert';

import 'package:test/test.dart';
import 'package:ws/interface.dart';

void main() {
  group('MessageStream', () {
    test('should extract text from stream', () {
      expectLater(
        WebSocketMessagesStream(
          Stream<String>.value('A'),
        ).text,
        emits('A'),
      );
      expectLater(
        WebSocketMessagesStream(
          Stream<List<int>>.value(utf8.encode('A')),
        ).text,
        neverEmits(anything),
      );
    });

    test('should extract bytes from stream', () {
      expectLater(
        WebSocketMessagesStream(
          Stream<String>.value('A'),
        ).bytes,
        neverEmits(anything),
      );
      expectLater(
        WebSocketMessagesStream(
          Stream<List<int>>.value(utf8.encode('A')),
        ).bytes,
        emits(utf8.encode('A')),
      );
    });

    test('should parse JSON data from the stream', () {
      expectLater(
        WebSocketMessagesStream(
          Stream<String>.value('{"a": 1, "b": null}'),
        ).json,
        emits({'a': 1, 'b': null}),
      );
      expectLater(
        WebSocketMessagesStream(
          Stream<List<int>>.value(utf8.encode('{"a": 1, "b": null}')),
        ).json,
        emits({'a': 1, 'b': null}),
      );
    });
  });
}

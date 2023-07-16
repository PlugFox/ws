import 'dart:async';
import 'dart:convert';

/// {@nodoc}
final Converter<String, Map<String, Object?>> _$jsonTextDecoder =
    const JsonDecoder().cast<String, Map<String, Object?>>();

/// {@nodoc}
final Converter<List<int>, Map<String, Object?>> _$jsonBytesDecoder =
    const Utf8Decoder().fuse<Map<String, Object?>>(
        const JsonDecoder().cast<String, Map<String, Object?>>());

/// Stream of message events handled by this WebSocket.
/// {@category Client}
/// {@category Entity}
final class WebSocketMessagesStream
    extends StreamView< /* String || List<int> */ Object> {
  /// Stream of message events handled by this WebSocket.
  WebSocketMessagesStream(super.stream);

  /// Filtered stream of binary data.
  late final Stream<List<int>> bytes = whereType<List<int>>();

  /// Filtered stream of text data.
  late final Stream<String> text = whereType<String>();

  /// Try to parse JSON data from the stream.
  /// If data is a valid JSON Map, the stream will emit a Map<String, Object?>.
  /// If data is not a valid JSON Map, the stream will not emit any data.
  /// If data is a valid JSON array, the stream will not emit any data.
  late final Stream<Map<String, Object?>> json =
      transform<Map<String, Object?>>(
    StreamTransformer<Object, Map<String, Object?>>.fromHandlers(
      handleData: (data, sink) {
        try {
          final json = switch (data) {
            String text
                when text.length >= 2 &&
                    text.codeUnitAt(0) == 123 &&
                    text.codeUnitAt(text.length - 1) == 125 =>
              _$jsonTextDecoder.convert(text),
            List<int> bytes
                when bytes.length >= 2 &&
                    bytes.first == 123 &&
                    bytes.last == 125 =>
              _$jsonBytesDecoder.convert(bytes),
            _ => null,
          };
          if (json != null) sink.add(json);
        } on Object {
          /* Do nothing */
        }
      },
    ),
  ).asBroadcastStream();

  /// Filtered stream of data of type T.
  Stream<T> whereType<T>() =>
      transform<T>(StreamTransformer<Object, T>.fromHandlers(
        handleData: (data, sink) => switch (data) {
          T valid => sink.add(valid),
          _ => null,
        },
      )).asBroadcastStream();
}

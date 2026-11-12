import 'dart:convert';
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/protocol/utf8_text.dart';
import 'package:ws/src/protocol/utf8_validator.dart';
import 'package:ws/src/transport/transport.dart';

const bool _isWeb = bool.fromEnvironment('dart.library.js_interop');

/// A text message was not valid UTF-8: a protocol error (close 1007), not a
/// recoverable decode failure.
final class InvalidUtf8 implements Exception {
  /// Creates the marker.
  const InvalidUtf8();
}

/// Converts between transport payloads and the client's message type.
/// Internal: the public API exposes only the three client flavours.
abstract base class MessageCodec<T> {
  /// Base constructor.
  const MessageCodec();

  /// What the transport should deliver for text messages.
  TransportTextMode get textMode => TransportTextMode.raw;

  /// Decodes a text message given as UTF-8 bytes.
  T fromTextBytes(Uint8List bytes);

  /// Decodes a text message given as a string.
  T fromTextString(String text);

  /// Accepts a value the transport already decoded from JSON.
  T fromJson(Object? value) =>
      throw const WebSocketFormatException('Unexpected decoded JSON');

  /// Decodes a binary message.
  T fromBinary(Uint8List bytes);

  /// Encodes and queues [data].
  void send(TransportConnection connection, T data);
}

/// `Uint8List` messages. Text messages arrive as their UTF-8 bytes.
final class BinaryCodec extends MessageCodec<Uint8List> {
  /// Creates the codec.
  const BinaryCodec();

  @override
  Uint8List fromTextBytes(Uint8List bytes) => bytes;

  @override
  Uint8List fromTextString(String text) => utf8.encode(text);

  @override
  Uint8List fromBinary(Uint8List bytes) => bytes;

  @override
  void send(TransportConnection connection, Uint8List data) =>
      connection.sendBinary(data);
}

/// `String` messages. Binary messages are decoded as UTF-8.
final class TextCodec extends MessageCodec<String> {
  /// Creates the codec.
  const TextCodec();

  @override
  String fromTextBytes(Uint8List bytes) {
    try {
      return decodeUtf8(bytes);
    } on FormatException {
      throw const InvalidUtf8();
    }
  }

  @override
  String fromTextString(String text) => text;

  @override
  String fromBinary(Uint8List bytes) {
    try {
      return decodeUtf8(bytes);
    } on FormatException catch (e) {
      throw WebSocketFormatException('Binary message is not UTF-8', cause: e);
    }
  }

  @override
  void send(TransportConnection connection, String data) {
    if (_isWeb) return connection.sendText(data);
    connection.sendTextBytes(encodeText(data));
  }

  /// `ascii.encode` is 2.2× faster than `utf8.encode` for ASCII strings and
  /// throws on the first non-ASCII character (DECISIONS.md D20).
  static Uint8List encodeText(String data) {
    try {
      return ascii.encode(data);
      // ignore: avoid_catching_errors
    } on ArgumentError {
      return utf8.encode(data);
    }
  }
}

final Converter<List<int>, Object?> _utf8Json = utf8.decoder.fuse(json.decoder);

/// `Map<String, Object?>` messages (JSON objects).
final class JsonCodec extends MessageCodec<Map<String, Object?>> {
  /// Creates the codec.
  const JsonCodec();

  @override
  TransportTextMode get textMode => TransportTextMode.json;

  @override
  Map<String, Object?> fromTextBytes(Uint8List bytes) {
    final Object? value;
    try {
      value = _utf8Json.convert(bytes);
    } on FormatException catch (e) {
      if (!Utf8Validator.validate(bytes)) throw const InvalidUtf8();
      throw WebSocketFormatException('Invalid JSON', cause: e);
    }
    return fromJson(value);
  }

  @override
  Map<String, Object?> fromTextString(String text) {
    final Object? value;
    try {
      value = json.decode(text);
    } on FormatException catch (e) {
      throw WebSocketFormatException('Invalid JSON', cause: e);
    }
    return fromJson(value);
  }

  @override
  Map<String, Object?> fromJson(Object? value) => switch (value) {
    final Map<String, Object?> map => map,
    _ => throw WebSocketFormatException(
      'Expected a JSON object, got ${value.runtimeType}',
    ),
  };

  @override
  Map<String, Object?> fromBinary(Uint8List bytes) =>
      throw const WebSocketFormatException('Binary message on a JSON client');

  static final JsonUtf8Encoder _encoder = JsonUtf8Encoder();

  @override
  void send(TransportConnection connection, Map<String, Object?> data) {
    if (_isWeb) return connection.sendText(json.encode(data));
    final List<int> bytes;
    try {
      bytes = _encoder.convert(data);
      // ignore: avoid_catching_errors
    } on JsonUnsupportedObjectError catch (e) {
      throw WebSocketFormatException('Cannot encode message as JSON', cause: e);
    }
    connection.sendTextBytes(
      bytes is Uint8List ? bytes : Uint8List.fromList(bytes),
    );
  }
}

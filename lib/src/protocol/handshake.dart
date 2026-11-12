import 'dart:convert' show ascii, base64, latin1;
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:ws/src/protocol/sha1.dart';

const String _guid = '258EAFA5-E914-47DA-95CA-C5AB0DC85B11';

/// `Sec-WebSocket-Accept` for a `Sec-WebSocket-Key` (RFC 6455 §4.2.2).
String acceptKey(String key) => base64.encode(sha1(ascii.encode(key + _guid)));

/// A client opening handshake request (RFC 6455 §4.1).
final class HandshakeRequest {
  /// Builds the request for [uri].
  HandshakeRequest(
    this.uri, {
    Map<String, String>? headers,
    Iterable<String>? protocols,
    math.Random? random,
  }) : headers = headers ?? const <String, String>{},
       protocols = List<String>.unmodifiable(protocols ?? const <String>[]),
       key = base64.encode(
         List<int>.generate(
           16,
           (_) => (random ?? math.Random.secure()).nextInt(256),
         ),
       );

  /// Target `ws://` or `wss://` URI.
  final Uri uri;

  /// Extra headers. Reserved handshake headers are rejected.
  final Map<String, String> headers;

  /// Subprotocols offered in `Sec-WebSocket-Protocol`.
  final List<String> protocols;

  /// The random `Sec-WebSocket-Key`.
  final String key;

  static const Set<String> _reserved = <String>{
    'host',
    'upgrade',
    'connection',
    'sec-websocket-key',
    'sec-websocket-version',
    'sec-websocket-accept',
    'sec-websocket-extensions',
    'sec-websocket-protocol',
    'content-length',
    'transfer-encoding',
  };

  /// Whether the connection uses TLS.
  bool get secure => uri.scheme == 'wss' || uri.scheme == 'https';

  /// The TCP port.
  int get port => uri.hasPort ? uri.port : (secure ? 443 : 80);

  /// Encodes the HTTP/1.1 request.
  Uint8List encode() {
    final defaultPort = port == (secure ? 443 : 80);
    final host = uri.host.contains(':') ? '[${uri.host}]' : uri.host;
    final path = uri.path.isEmpty ? '/' : uri.path;
    final target = uri.hasQuery ? '$path?${uri.query}' : path;
    final b = StringBuffer()
      ..write('GET $target HTTP/1.1\r\n')
      ..write('Host: ${defaultPort ? host : '$host:$port'}\r\n')
      ..write('Upgrade: websocket\r\n')
      ..write('Connection: Upgrade\r\n')
      ..write('Sec-WebSocket-Key: $key\r\n')
      ..write('Sec-WebSocket-Version: 13\r\n');
    if (protocols.isNotEmpty) {
      b.write('Sec-WebSocket-Protocol: ${protocols.join(', ')}\r\n');
    }
    if (uri.userInfo.isNotEmpty) {
      b.write(
        'Authorization: Basic '
        '${base64.encode(latin1.encode(Uri.decodeComponent(uri.userInfo)))}'
        '\r\n',
      );
    }
    for (final MapEntry(:key, :value) in headers.entries) {
      if (_reserved.contains(key.toLowerCase())) {
        throw ArgumentError.value(key, 'headers', 'Reserved handshake header');
      }
      if (_hasLineBreak(key) || _hasLineBreak(value)) {
        throw ArgumentError.value(key, 'headers', 'Header contains CR or LF');
      }
      b.write('$key: $value\r\n');
    }
    b.write('\r\n');
    return latin1.encode(b.toString());
  }

  static bool _hasLineBreak(String s) => s.contains('\r') || s.contains('\n');
}

/// The server's handshake response.
final class HandshakeResponse {
  HandshakeResponse._(this.statusCode, this.reason, this.headers);

  /// HTTP status code.
  final int statusCode;

  /// HTTP reason phrase.
  final String reason;

  /// Response headers; names in lower case, repeated headers joined by `, `.
  final Map<String, String> headers;

  /// Index of the `\r\n\r\n` that ends the response head, or -1.
  static int headEnd(Uint8List bytes) {
    for (var i = 0; i + 3 < bytes.length; i++) {
      if (bytes[i] == 13 &&
          bytes[i + 1] == 10 &&
          bytes[i + 2] == 13 &&
          bytes[i + 3] == 10) {
        return i;
      }
    }
    return -1;
  }

  /// Parses the response head (without the final empty line).
  factory HandshakeResponse.parse(Uint8List head) {
    final lines = latin1.decode(head).split('\r\n');
    final status = lines.first.split(' ');
    if (status.length < 2 || !status[0].startsWith('HTTP/1.')) {
      throw const FormatException('Not an HTTP response');
    }
    final code = int.tryParse(status[1]);
    if (code == null) throw const FormatException('Invalid HTTP status');
    final headers = <String, String>{};
    for (final line in lines.skip(1)) {
      final i = line.indexOf(':');
      if (i <= 0) continue;
      final name = line.substring(0, i).trim().toLowerCase();
      final value = line.substring(i + 1).trim();
      headers.update(name, (v) => '$v, $value', ifAbsent: () => value);
    }
    return HandshakeResponse._(code, status.skip(2).join(' '), headers);
  }

  /// Checks the response against [request]. Returns the selected
  /// subprotocol, or throws [HandshakeError].
  String? validate(HandshakeRequest request) {
    if (statusCode != 101) {
      throw HandshakeError(
        'Unexpected HTTP status $statusCode $reason',
        statusCode: statusCode,
        headers: headers,
      );
    }
    bool hasToken(String name, String token) => (headers[name] ?? '')
        .toLowerCase()
        .split(',')
        .any((t) => t.trim() == token);
    if (!hasToken('upgrade', 'websocket')) {
      throw HandshakeError(
        'Missing "Upgrade: websocket"',
        statusCode: statusCode,
        headers: headers,
      );
    }
    if (!hasToken('connection', 'upgrade')) {
      throw HandshakeError(
        'Missing "Connection: Upgrade"',
        statusCode: statusCode,
        headers: headers,
      );
    }
    if (headers['sec-websocket-accept'] != acceptKey(request.key)) {
      throw HandshakeError(
        'Invalid Sec-WebSocket-Accept',
        statusCode: statusCode,
        headers: headers,
      );
    }
    if (headers['sec-websocket-extensions'] case final e? when e.isNotEmpty) {
      throw HandshakeError(
        'Server selected an extension that was not offered',
        statusCode: statusCode,
        headers: headers,
      );
    }
    final protocol = headers['sec-websocket-protocol'];
    if (protocol != null && !request.protocols.contains(protocol)) {
      throw HandshakeError(
        'Server selected an unknown subprotocol $protocol',
        statusCode: statusCode,
        headers: headers,
      );
    }
    return protocol;
  }
}

/// The opening handshake failed.
final class HandshakeError implements Exception {
  /// Creates the error.
  const HandshakeError(
    this.message, {
    this.statusCode,
    this.headers = const <String, String>{},
  });

  /// Description.
  final String message;

  /// HTTP status of the response, if one was received.
  final int? statusCode;

  /// Response headers, lower-case names.
  final Map<String, String> headers;

  @override
  String toString() => 'HandshakeError: $message';
}

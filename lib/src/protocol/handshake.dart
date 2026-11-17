import 'dart:convert' show ascii, base64, latin1, utf8;
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

  /// Throws [ArgumentError] for a host name this library cannot resolve:
  /// a non-ASCII (IDN) name, which `Uri` percent-encodes. Browsers convert
  /// it themselves; elsewhere give it in punycode
  /// (docs/review/protocol.md F17).
  static void checkHost(Uri uri) {
    if (!uri.host.contains(':') && uri.host.contains('%')) {
      throw ArgumentError.value(
        uri.toString(),
        'url',
        'Give non-ASCII host names in punycode (xn--...)',
      );
    }
  }

  /// The host to open a socket to: an IPv6 zone id (`%25eth0` in the URL)
  /// decoded to `%eth0`.
  static String socketHost(Uri uri) =>
      uri.host.contains(':') ? Uri.decodeComponent(uri.host) : uri.host;

  /// Encodes the HTTP/1.1 request.
  Uint8List encode() {
    final defaultPort = port == (secure ? 443 : 80);
    // A zone id is local to this machine: never sent (RFC 6874 §4).
    final ip = uri.host.split('%').first;
    final host = uri.host.contains(':') ? '[$ip]' : uri.host;
    final path = uri.path.isEmpty ? '/' : uri.path;
    final target = uri.hasQuery ? '$path?${uri.query}' : path;
    final b = StringBuffer()
      ..write('GET $target HTTP/1.1\r\n')
      ..write('Host: ${defaultPort ? host : '$host:$port'}\r\n')
      ..write('Upgrade: websocket\r\n')
      ..write('Connection: Upgrade\r\n')
      ..write('Sec-WebSocket-Key: $key\r\n')
      ..write('Sec-WebSocket-Version: 13\r\n');
    validateProtocols(protocols);
    if (protocols.isNotEmpty) {
      b.write('Sec-WebSocket-Protocol: ${protocols.join(', ')}\r\n');
    }
    if (urlAuthorization(uri, headers) case final authorization?) {
      b.write('Authorization: $authorization\r\n');
    }
    validateHeaders(headers);
    for (final MapEntry(:key, :value) in headers.entries) {
      b.write('$key: $value\r\n');
    }
    b.write('\r\n');
    return latin1.encode(b.toString());
  }

  /// [basicAuthorization] unless [headers] already set `Authorization`
  /// (any case): an explicit header wins, and only one is sent.
  static String? urlAuthorization(Uri uri, Map<String, String> headers) =>
      headers.keys.any((k) => k.toLowerCase() == 'authorization')
      ? null
      : basicAuthorization(uri);

  /// `Basic` credentials from the URL's user info (UTF-8, RFC 7617), or
  /// `null`. Every transport sends them, as browsers do.
  static String? basicAuthorization(Uri uri) {
    if (uri.userInfo.isEmpty) return null;
    final credentials = utf8.encode(Uri.decodeComponent(uri.userInfo));
    return 'Basic ${base64.encode(credentials)}';
  }

  /// Throws [ArgumentError] unless every subprotocol is an RFC 9110 token
  /// (RFC 6455 §4.1), which also rules out header injection.
  static void validateProtocols(List<String> protocols) {
    for (final protocol in protocols) {
      if (!_isToken(protocol)) {
        throw ArgumentError.value(
          protocol,
          'protocols',
          'A subprotocol must be an RFC 9110 token',
        );
      }
    }
  }

  /// Throws [ArgumentError] for a header no transport may send: reserved
  /// handshake headers, names that are not RFC 9110 tokens, values with
  /// control characters or non-ASCII.
  static void validateHeaders(Map<String, String> headers) {
    for (final MapEntry(:key, :value) in headers.entries) {
      if (_reserved.contains(key.toLowerCase())) {
        throw ArgumentError.value(key, 'headers', 'Reserved handshake header');
      }
      if (!_isToken(key)) {
        throw ArgumentError.value(key, 'headers', 'Invalid header name');
      }
      if (!_isVisibleAscii(value)) {
        throw ArgumentError.value(
          key,
          'headers',
          'Header value must be visible ASCII (no CR, LF or non-ASCII)',
        );
      }
    }
  }

  /// RFC 9110 field values as every transport can send them: visible
  /// ASCII, space and tab (tungstenite rejects the rest too).
  static bool _isVisibleAscii(String s) {
    for (var i = 0; i < s.length; i++) {
      final c = s.codeUnitAt(i);
      if (c != 0x09 && (c < 0x20 || c > 0x7E)) return false;
    }
    return true;
  }

  /// RFC 9110 token (header names).
  static bool _isToken(String s) {
    if (s.isEmpty) return false;
    for (var i = 0; i < s.length; i++) {
      final c = s.codeUnitAt(i);
      final ok =
          (c >= 0x30 && c <= 0x39) ||
          (c >= 0x41 && c <= 0x5A) ||
          (c >= 0x61 && c <= 0x7A) ||
          r"!#$%&'*+-.^_`|~".codeUnits.contains(c);
      if (!ok) return false;
    }
    return true;
  }
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

  static final RegExp _statusLine = RegExp(
    r'^HTTP/1\.([01]) ([0-9]{3})(?: (.*))?$',
  );
  static final RegExp _token = RegExp(r"^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$");

  /// Whether [head] is an informational response other than 101 (`100`,
  /// `103 Early Hints`), which precedes the real one.
  static bool isInterim(Uint8List head) {
    final status = _statusLine.firstMatch(
      latin1
          .decode(Uint8List.sublistView(head, 0, math.min(head.length, 64)))
          .split('\r\n')
          .first,
    );
    final code = status == null ? 0 : int.parse(status[2]!);
    return code >= 100 && code < 200 && code != 101;
  }

  /// Parses the response head (without the final empty line), as strictly
  /// as RFC 9112 asks: `HTTP/1.1` (1.0 only for a refusal), a three-digit
  /// status, header names without surrounding whitespace, no obsolete line
  /// folding. A response a strict intermediary would read differently is
  /// refused (docs/review/protocol.md F15).
  factory HandshakeResponse.parse(Uint8List head) {
    final lines = latin1.decode(head).split('\r\n');
    final status = _statusLine.firstMatch(lines.first);
    if (status == null) throw const FormatException('Not an HTTP response');
    final code = int.parse(status[2]!);
    if (code == 101 && status[1] != '1') {
      throw const FormatException('Switching protocols needs HTTP/1.1');
    }
    final headers = <String, String>{};
    for (final line in lines.skip(1)) {
      if (line.startsWith(' ') || line.startsWith('\t')) {
        throw const FormatException('Obsolete line folding');
      }
      final i = line.indexOf(':');
      final name = i < 0 ? '' : line.substring(0, i);
      if (!_token.hasMatch(name)) {
        throw FormatException('Invalid header line', line);
      }
      final value = line.substring(i + 1).trim();
      headers.update(
        name.toLowerCase(),
        (v) => '$v, $value',
        ifAbsent: () => value,
      );
    }
    return HandshakeResponse._(code, status[3] ?? '', headers);
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

import 'dart:async';
import 'dart:convert';
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/transport/transport.dart';

/// An in-memory WebSocket server for tests. Pass it as
/// `WebSocketOptions.transport`; every `connect` creates a
/// [FakeWebSocketConnection] that the test drives.
///
/// Events reach the client asynchronously (in a microtask), like on a real
/// network, so tests can use `fake_async` or plain `await`.
///
/// ```dart
/// final server = FakeWebSocketServer();
/// final client = WebSocketClient.text(
///   options: WebSocketOptions(transport: server),
/// );
/// await client.connect('ws://test');
/// server.last.sendText('hello');
/// ```
final class FakeWebSocketServer implements WebSocketTransport {
  /// Creates a server. With [autoAccept] every connection opens right away;
  /// otherwise the test calls [FakeWebSocketConnection.accept] or
  /// [FakeWebSocketConnection.reject].
  FakeWebSocketServer({this.autoAccept = true, this.protocol});

  /// Whether new connections open immediately.
  bool autoAccept;

  /// Subprotocol selected when connections are accepted automatically.
  String? protocol;

  final List<FakeWebSocketConnection> _connections =
      <FakeWebSocketConnection>[];
  final StreamController<FakeWebSocketConnection> _connected =
      StreamController<FakeWebSocketConnection>.broadcast(sync: true);

  /// Every connection made so far, oldest first.
  List<FakeWebSocketConnection> get connections =>
      List<FakeWebSocketConnection>.unmodifiable(_connections);

  /// The newest connection.
  FakeWebSocketConnection get last => _connections.last;

  /// New connections, as they are made.
  Stream<FakeWebSocketConnection> get connected => _connected.stream;

  @override
  String get name => 'fake';

  @override
  bool get supportsPing => true;

  @override
  bool get decodesJson => false;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) {
    final connection = FakeWebSocketConnection._(request, listener);
    _connections.add(connection);
    _connected.add(connection);
    if (autoAccept) connection.accept(protocol: protocol);
    return connection._client;
  }
}

/// The server side of one fake connection.
final class FakeWebSocketConnection {
  FakeWebSocketConnection._(this.request, this._listener) {
    _client = _FakeClientEnd(this);
  }

  /// What the client asked for.
  final TransportRequest request;
  final TransportListener _listener;
  late final _FakeClientEnd _client;

  final List<Object> _received = <Object>[];
  final StreamController<Object> _incoming = StreamController<Object>.broadcast(
    sync: true,
  );

  bool _accepted = false;
  bool _ended = false;

  /// Answer client pings automatically.
  bool autoPong = true;

  /// Answer the client's close frame automatically. Set to `false` to test
  /// the disconnect timeout.
  bool echoClose = true;

  /// Pings received from the client.
  int pings = 0;

  /// What the client sees as its send queue size. Set it above the client's
  /// `sendHighWaterMark` to make `push()` wait, then call [drain].
  int bufferedAmount = 0;

  bool _drainRequested = false;

  /// Empties the simulated send queue; a waiting `push()` completes.
  void drain() {
    bufferedAmount = 0;
    if (_drainRequested) {
      _drainRequested = false;
      _later(_listener.onDrain);
    }
  }

  /// The close code and reason the client sent, once it did.
  ({int? code, String? reason})? clientClose;

  /// Messages from the client: `String` for text, `Uint8List` for binary.
  List<Object> get received => List<Object>.unmodifiable(_received);

  /// Messages from the client as they arrive.
  Stream<Object> get messages => _incoming.stream;

  /// Text messages from the client decoded as JSON.
  Iterable<Object?> get receivedJson =>
      _received.whereType<String>().map(jsonDecode);

  /// Whether the connection is open (accepted and not ended).
  bool get isOpen => _accepted && !_ended;

  /// Whether the connection ended.
  bool get isEnded => _ended;

  void _later(void Function() fn) => scheduleMicrotask(() {
    if (!_ended) fn();
  });

  /// Completes the handshake.
  void accept({String? protocol}) {
    if (_accepted || _ended) return;
    _accepted = true;
    _later(() => _listener.onOpen(protocol));
  }

  /// Fails the handshake with HTTP [statusCode].
  void reject(int statusCode, {Map<String, String> headers = const {}}) => fail(
    WebSocketHandshakeException(
      'Unexpected HTTP status $statusCode',
      statusCode: statusCode,
      headers: headers,
    ),
  );

  /// Ends the connection with [error] (e.g. a network failure).
  void fail(WebSocketException error) {
    if (_ended) return;
    _ended = true;
    scheduleMicrotask(() => _listener.onClosed(null, '', error));
  }

  /// The connection is lost without a close frame (code 1006).
  void drop() =>
      fail(const WebSocketConnectionException('Connection lost (fake)'));

  /// Sends a text message to the client.
  void sendText(String text) {
    _requireOpen();
    final bytes = utf8.encode(text);
    _later(
      () => request.textMode == TransportTextMode.json && _client._decodeJson
          ? _listener.onJson(jsonDecode(text))
          : _listener.onTextBytes(bytes),
    );
  }

  /// Sends [value] encoded as JSON text.
  void sendJson(Object? value) => sendText(jsonEncode(value));

  /// Sends raw text bytes (may be invalid UTF-8, for tests).
  void sendTextBytes(Uint8List bytes) {
    _requireOpen();
    _later(() => _listener.onTextBytes(bytes));
  }

  /// Sends a binary message.
  void sendBinary(Uint8List bytes) {
    _requireOpen();
    _later(() => _listener.onBinary(bytes));
  }

  /// Sends a pong.
  void pong() {
    _requireOpen();
    _later(_listener.onPong);
  }

  /// Starts a server-side close; the connection ends with [code].
  void close([int? code = 1000, String reason = '']) {
    if (_ended) return;
    _ended = true;
    scheduleMicrotask(() => _listener.onClosed(code, reason, null));
  }

  void _requireOpen() {
    if (!isOpen) throw StateError('The fake connection is not open');
  }

  void _fromClient(Object message) {
    _received.add(message);
    _incoming.add(message);
  }
}

final class _FakeClientEnd implements TransportConnection {
  _FakeClientEnd(this._server);

  final FakeWebSocketConnection _server;
  bool _closing = false;

  /// The fake does not decode JSON for the client; it delivers bytes like
  /// the pure-Dart transport.
  bool get _decodeJson => false;

  void _check() {
    if (!_server.isOpen || _closing) {
      throw const WebSocketNotConnectedException('The connection is closing');
    }
  }

  @override
  int get bufferedAmount => _server.bufferedAmount;

  @override
  void requestDrain() {
    if (_server.bufferedAmount == 0) {
      _server._later(_server._listener.onDrain);
    } else {
      _server._drainRequested = true;
    }
  }

  @override
  void sendText(String text) {
    _check();
    _server._fromClient(text);
  }

  @override
  void sendTextBytes(Uint8List bytes) {
    _check();
    _server._fromClient(utf8.decode(bytes));
  }

  @override
  void sendBinary(Uint8List bytes) {
    _check();
    _server._fromClient(Uint8List.fromList(bytes));
  }

  @override
  void ping() {
    _check();
    _server.pings++;
    if (_server.autoPong) _server._later(_server._listener.onPong);
  }

  @override
  void close(int? code, String? reason) {
    if (_closing || _server._ended) return;
    _closing = true;
    _server.clientClose = (code: code, reason: reason);
    if (!_server._accepted) return abort();
    if (_server.echoClose) _server.close(code, reason ?? '');
  }

  @override
  void abort() =>
      _server.fail(const WebSocketConnectionException('Connection aborted'));
}

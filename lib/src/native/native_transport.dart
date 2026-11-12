import 'dart:async';
import 'dart:convert';
import 'dart:ffi';
import 'dart:isolate';
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/native/native_library.dart';
import 'package:ws/src/native/tape.dart';
import 'package:ws/src/transport/transport.dart';

const int _evOpen = 0;
const int _evClosed = 1;
const int _evBatch = 3;
const int _evPong = 4;
const int _evDrain = 5;

const int _kindText = 0;
const int _kindBinary = 1;
const int _kindTape = 2;

const int _errNone = 0;
const int _errHandshake = 2;
const int _errProtocol = 3;
const int _errTls = 4;

/// The Rust transport: tokio + tungstenite + rustls, JSON parsed into a
/// tape off the Dart thread, messages delivered in batches as external
/// typed data (no copies), receive credits for flow control.
final class NativeTransport implements WebSocketTransport {
  /// Creates the transport on [library], whose runtime must be started
  /// (`WsNativeLibrary.install`; `WsNativeLibrary.transport` checks that).
  const NativeTransport(this.library, {this.window = 8 << 20});

  /// The loaded Rust library.
  final WsNativeLibrary library;

  /// Receive window in bytes: how far Rust may read ahead of Dart.
  final int window;

  @override
  String get name => 'native';

  @override
  bool get supportsPing => true;

  @override
  bool get decodesJson => true;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) => _NativeConnection(library, request, listener, window);
}

final class _NativeConnection implements TransportConnection, Finalizable {
  _NativeConnection(
    this._lib,
    TransportRequest request,
    this._listener,
    int window,
  ) {
    _port = RawReceivePort(_onEvent, 'ws_native');
    final tls = request.tls;
    final config = utf8.encode(
      jsonEncode(<String, Object?>{
        'url': request.url.toString(),
        'headers': [
          for (final MapEntry(:key, :value) in request.headers.entries)
            [key, value],
        ],
        'protocols': request.protocols,
        'max_message_size': request.maxMessageSize,
        'json': request.textMode == TransportTextMode.json,
        'window': window,
        'allow_insecure': tls.allowInsecure,
        'roots_pem': tls.trustedRootsPem,
      }),
    );
    _id = _lib.connect(
      _lib.copy(config),
      config.length,
      _port.sendPort.nativePort,
    );
    if (_id == 0) {
      _port.close();
      _finished = true;
      scheduleMicrotask(
        () => _listener.onClosed(
          null,
          '',
          const WebSocketConnectionException(
            'The native transport rejected the connection request',
          ),
        ),
      );
      return;
    }
    _lib.finalizer.attach(this, Pointer<Void>.fromAddress(_id), detach: this);
  }

  final WsNativeLibrary _lib;
  final TransportListener _listener;
  late final RawReceivePort _port;
  late final int _id;
  bool _finished = false;

  /// After `abort()` only the final event is delivered.
  bool _aborted = false;

  void _onEvent(Object? event) {
    if (event == null || _finished) return; // liveness probe
    final e = event as List<Object?>;
    if (_aborted && e[0] != _evClosed) return;
    switch (e[0]) {
      case _evBatch:
        try {
          for (var i = 2; i < e.length && !_finished && !_aborted; i += 2) {
            final item = e[i + 1]! as Uint8List;
            switch (e[i]) {
              case _kindText:
                _listener.onTextBytes(item);
              case _kindBinary:
                _listener.onBinary(item);
              case _kindTape:
                _listener.onJson(TapeDocument(item).root);
            }
          }
        } finally {
          // Credits go back after the batch was handled, even if a listener
          // threw: otherwise the reader would stall for good.
          if (!_finished) _lib.grant(_id, e[1]! as int);
        }
      case _evOpen:
        _listener.onOpen(e[1] as String?);
      case _evPong:
        _listener.onPong();
      case _evDrain:
        _listener.onDrain();
      case _evClosed:
        _finished = true;
        _port.close();
        _lib.finalizer.detach(this);
        _listener.onClosed(e[1] as int?, e[2]! as String, _error(e));
    }
  }

  static WebSocketException? _error(List<Object?> e) {
    final kind = e[3]! as int;
    final message = e[4] as String? ?? 'Connection failed';
    return switch (kind) {
      _errNone => null,
      _errHandshake => WebSocketHandshakeException(
        message,
        statusCode: e[5] as int?,
        headers: switch (e[6]) {
          final String json => (jsonDecode(json) as Map<String, Object?>).map(
            (k, v) => MapEntry(k.toLowerCase(), '$v'),
          ),
          _ => const <String, String>{},
        },
      ),
      _errProtocol => WebSocketClosedException(
        'Protocol violation by the server: $message',
        code: e[1] as int?,
        reason: e[2]! as String,
        byServer: false,
      ),
      _errTls => WebSocketConnectionException('TLS failed: $message'),
      _ => WebSocketConnectionException(message),
    };
  }

  void _send(Uint8List bytes, {required bool binary}) {
    if (_finished) {
      throw const WebSocketNotConnectedException('The connection is closed');
    }
    switch (_lib.send(_id, _lib.copy(bytes), bytes.length, binary)) {
      case 0:
        return;
      case 2:
        throw const WebSocketFormatException('Text is not valid UTF-8');
      default:
        throw const WebSocketNotConnectedException('The connection is closed');
    }
  }

  @override
  void sendText(String text) => _send(utf8.encode(text), binary: false);

  @override
  void sendTextBytes(Uint8List bytes) => _send(bytes, binary: false);

  @override
  void sendBinary(Uint8List bytes) => _send(bytes, binary: true);

  @override
  void ping() {
    if (!_finished) _lib.ping(_id);
  }

  @override
  void close(int? code, String? reason) {
    if (_finished) return;
    final bytes = utf8.encode(reason ?? '');
    _lib.close(_id, code ?? -1, _lib.copy(bytes), bytes.length);
  }

  @override
  void abort() {
    if (_finished) return;
    _aborted = true;
    _lib.abort(_id);
  }

  @override
  int get bufferedAmount => _finished ? 0 : _lib.buffered(_id);

  @override
  void requestDrain() {
    if (!_finished) _lib.requestDrain(_id);
  }
}

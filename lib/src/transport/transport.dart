import 'dart:typed_data';

import 'package:meta/meta.dart';
import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/client/options.dart';

/// What a transport should deliver for incoming text messages.
enum TransportTextMode {
  /// UTF-8 bytes ([TransportListener.onTextBytes]), or a `String` where the
  /// platform hands out strings anyway (browser).
  raw,

  /// Decoded JSON ([TransportListener.onJson]) when the transport can decode
  /// off the Dart thread (native: simd-json tape); otherwise [raw].
  json,
}

/// Everything a transport needs to open one connection.
@immutable
final class TransportRequest {
  /// Creates a request.
  const TransportRequest({
    required this.url,
    this.headers = const <String, String>{},
    this.protocols = const <String>[],
    this.maxMessageSize = 64 << 20,
    this.textMode = TransportTextMode.raw,
    this.tls = const WebSocketTls(),
  });

  /// `ws://` or `wss://` URL.
  final Uri url;

  /// Handshake headers.
  final Map<String, String> headers;

  /// Offered subprotocols.
  final List<String> protocols;

  /// Largest incoming message in bytes.
  final int maxMessageSize;

  /// Delivery of text messages.
  final TransportTextMode textMode;

  /// TLS settings.
  final WebSocketTls tls;
}

/// Receives the events of one connection.
///
/// Contract for every transport: callbacks run on the isolate that called
/// `connect`, never synchronously from a `TransportConnection` method;
/// [onOpen] at most once; exactly one [onClosed], always last.
abstract interface class TransportListener {
  /// The handshake succeeded.
  void onOpen(String? protocol);

  /// A text message as UTF-8 bytes. Not validated: the decoder validates
  /// (DECISIONS.md D5), and a decode failure is a protocol error 1007.
  void onTextBytes(Uint8List bytes);

  /// A text message as a string (browser).
  void onTextString(String text);

  /// A text message already decoded from JSON by the transport.
  void onJson(Object? value);

  /// A binary message.
  void onBinary(Uint8List bytes);

  /// A pong arrived (protocol heartbeat).
  void onPong();

  /// The send queue is empty, after [TransportConnection.requestDrain].
  void onDrain();

  /// The connection ended. [error] is set when it failed rather than
  /// closed with a close frame.
  void onClosed(int? code, String reason, WebSocketException? error);
}

/// One open or opening connection.
abstract interface class TransportConnection {
  /// Queues a text message given as a string.
  void sendText(String text);

  /// Queues a text message given as UTF-8 bytes (ownership passes to the
  /// transport; the caller must not modify [bytes] afterwards).
  void sendTextBytes(Uint8List bytes);

  /// Queues a binary message. [bytes] is copied or sent before return; the
  /// caller may reuse it.
  void sendBinary(Uint8List bytes);

  /// Sends a protocol ping. Throws [WebSocketUnsupportedException] where
  /// pings are not available.
  void ping();

  /// Starts the closing handshake.
  void close(int? code, String? reason);

  /// Drops the connection immediately, without a closing handshake.
  /// `onClosed` still follows.
  void abort();

  /// Bytes queued but not yet handed to the operating system.
  int get bufferedAmount;

  /// Asks for one [TransportListener.onDrain] when [bufferedAmount] reaches
  /// zero (soon, if it already is). Drain events are on request only: a
  /// native transport would otherwise post one per message sent.
  void requestDrain();
}

/// Opens connections. One instance can serve many clients.
abstract interface class WebSocketTransport {
  /// A short name for logs and metrics (`dart`, `web`, `native`, `fake`).
  String get name;

  /// Whether this transport can send protocol pings.
  bool get supportsPing;

  /// Whether [TransportTextMode.json] decodes off the Dart thread.
  bool get decodesJson;

  /// Starts connecting; events arrive at [listener].
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  );
}

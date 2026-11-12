/// Every error the client reports is a [WebSocketException]. Raw
/// `SocketException`s, browser events or native error strings never reach
/// the user; they are kept as [WebSocketException.cause].
///
/// The set of subclasses is fixed for 2.x, so a `switch` over them can be
/// exhaustive; finer reasons are enums inside them
/// ([WebSocketConnectionFailure], [WebSocketTlsFailure],
/// [WebSocketRpcFailure]).
///
/// `dart:io` also declares a `WebSocketException`; in a file that imports
/// both, the one from this package wins silently. Import this package with
/// a prefix there (`import 'package:ws/ws.dart' as ws;`).
sealed class WebSocketException implements Exception {
  const WebSocketException(this.message, {this.cause});

  /// Description of the error.
  final String message;

  /// The underlying error, if any (for logs only; its type is not part of
  /// the API).
  final Object? cause;

  String get _name;

  @override
  String toString() =>
      '$_name: $message${cause == null ? '' : ' (cause: $cause)'}';
}

/// The server answered the opening handshake with something other than a
/// WebSocket upgrade. Browsers hide the HTTP status, so on the web this is
/// reported as a [WebSocketConnectionException] instead.
final class WebSocketHandshakeException extends WebSocketException {
  /// Creates the exception.
  const WebSocketHandshakeException(
    super.message, {
    this.statusCode,
    this.headers = const <String, String>{},
    super.cause,
  });

  /// HTTP status, when a response was received.
  final int? statusCode;

  /// Response headers, names in lower case.
  final Map<String, String> headers;

  @override
  String get _name => 'WebSocketHandshakeException';
}

/// What failed in a [WebSocketConnectionException].
///
/// New values may be added in minor releases: give a `switch` over it a
/// default branch.
enum WebSocketConnectionFailure {
  /// The host name did not resolve (offline, wrong name, DNS blocked).
  dns,

  /// The server actively refused the TCP connection (nothing listens on
  /// the port).
  refused,

  /// No route, no answer or another socket error while connecting.
  unreachable,

  /// TLS failed; see [WebSocketConnectionException.tls].
  tls,

  /// The HTTP proxy was unreachable or refused the tunnel; see
  /// [WebSocketConnectionException.proxyStatus].
  proxy,

  /// An established connection was dropped without a close frame (reset,
  /// end of stream, keepalive timeout).
  lost,

  /// Anything else, including every connect failure in the browser, which
  /// does not say why.
  other,
}

/// Why TLS failed. Certificate failures are retried at the maximum
/// reconnect delay: a captive portal or a TLS-inspecting network causes
/// them, and they rarely fix themselves quickly.
///
/// New values may be added in minor releases: give a `switch` over it a
/// default branch.
enum WebSocketTlsFailure {
  /// The certificate chain does not lead to a trusted root (self-signed,
  /// private CA, TLS inspection).
  untrusted,

  /// The certificate is not valid for the host name.
  hostname,

  /// The certificate has expired, or the device clock is wrong.
  expired,

  /// The certificate is not valid yet: the device clock is probably
  /// behind.
  notYetValid,

  /// The certificate was revoked.
  revoked,

  /// The handshake itself failed: no common protocol version or cipher, an
  /// alert from the server, or a server that does not speak TLS.
  handshake,

  /// Anything else (configuration, unclassified platform errors).
  other;

  /// Whether the server's certificate was rejected (as opposed to the
  /// handshake failing).
  bool get isCertificate => switch (this) {
    untrusted || hostname || expired || notYetValid || revoked => true,
    handshake || other => false,
  };
}

/// DNS, TCP, TLS or proxy failure while connecting, or an established
/// connection lost without a close frame. [failure] says which.
final class WebSocketConnectionException extends WebSocketException {
  /// Creates the exception.
  const WebSocketConnectionException(
    super.message, {
    this.failure = WebSocketConnectionFailure.other,
    this.tls,
    this.proxyStatus,
    super.cause,
  });

  /// What failed.
  final WebSocketConnectionFailure failure;

  /// Why TLS failed, when [failure] is [WebSocketConnectionFailure.tls].
  final WebSocketTlsFailure? tls;

  /// The proxy's HTTP status (for example 407: authentication required),
  /// when [failure] is [WebSocketConnectionFailure.proxy] and the proxy
  /// answered.
  final int? proxyStatus;

  @override
  String get _name => switch ((failure, tls, proxyStatus)) {
    (WebSocketConnectionFailure.tls, final tls?, _) =>
      'WebSocketConnectionException(tls: ${tls.name})',
    (WebSocketConnectionFailure.proxy, _, final status?) =>
      'WebSocketConnectionException(proxy: $status)',
    (WebSocketConnectionFailure.other, _, _) => 'WebSocketConnectionException',
    (final failure, _, _) => 'WebSocketConnectionException(${failure.name})',
  };
}

/// Connecting, the closing handshake or a heartbeat took too long.
final class WebSocketTimeoutException extends WebSocketException {
  /// Creates the exception.
  const WebSocketTimeoutException(super.message, {super.cause});

  @override
  String get _name => 'WebSocketTimeoutException';
}

/// The connection was closed with a close frame (by the server, or by the
/// client because the server broke the protocol or a limit).
final class WebSocketClosedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketClosedException(
    super.message, {
    this.code,
    this.reason = '',
    this.byServer = true,
    super.cause,
  });

  /// The close code; `null` when the close frame had none (1005).
  final int? code;

  /// The close reason.
  final String reason;

  /// Whether the server started the close.
  final bool byServer;

  @override
  String get _name => 'WebSocketClosedException';
}

/// An operation needs an open connection.
final class WebSocketNotConnectedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketNotConnectedException(super.message);

  @override
  String get _name => 'WebSocketNotConnectedException';
}

/// `push()` was refused because more than
/// `WebSocketOptions.sendBufferLimit` bytes wait to be sent. The connection
/// stays open; wait (or await earlier `push()` futures) and retry.
final class WebSocketBufferFullException extends WebSocketException {
  /// Creates the exception.
  const WebSocketBufferFullException(super.message, {required this.buffered});

  /// Bytes queued when the message was refused.
  final int buffered;

  @override
  String get _name => 'WebSocketBufferFullException';
}

/// A message could not be decoded or encoded. Incoming messages that fail
/// are dropped and reported through `onError`; the connection stays open.
final class WebSocketFormatException extends WebSocketException {
  /// Creates the exception.
  const WebSocketFormatException(super.message, {super.cause});

  @override
  String get _name => 'WebSocketFormatException';
}

/// The operation is not available on this platform or transport (for
/// example a protocol ping in the browser).
final class WebSocketUnsupportedException extends WebSocketException {
  /// Creates the exception.
  const WebSocketUnsupportedException(super.message);

  @override
  String get _name => 'WebSocketUnsupportedException';
}

/// Why an RPC call failed.
enum WebSocketRpcFailure {
  /// No reply within the timeout.
  timeout,

  /// The connection was lost or `disconnect()` was called before the reply.
  disconnected,

  /// The client was closed.
  closed,

  /// The server replied with an error.
  remote,
}

/// An RPC call failed. Calls are never retried automatically.
final class WebSocketRpcException extends WebSocketException {
  /// Creates the exception.
  const WebSocketRpcException(
    super.message, {
    required this.failure,
    this.id,
    this.error,
    super.cause,
  });

  /// What went wrong.
  final WebSocketRpcFailure failure;

  /// The request id, if one was assigned.
  final Object? id;

  /// The error payload from the server, for [WebSocketRpcFailure.remote].
  final Object? error;

  @override
  String get _name => 'WebSocketRpcException';
}

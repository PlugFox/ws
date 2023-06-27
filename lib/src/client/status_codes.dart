import 'package:meta/meta.dart';

/// Endpoints MAY use the following pre-defined status codes
/// when sending a Close frame.
/// [Web API](https://developer.mozilla.org/en-US/docs/Web/API/CloseEvent/code)
/// [RFC 6455, Section 7.4.1](https://tools.ietf.org/html/rfc6455#section-7.4.1)
///
/// 0..999 - Unused
/// Status codes in the range 0-999 are not used.
///
/// 1016..2999 - Reserved for websocket extensions
/// Status codes in the range 1000-2999 are reserved for definition by
/// this protocol, its future revisions, and extensions specified in a
/// permanent and readily available public specification.
///
/// 3000..3999 - Registered first come first serve at IANA
/// Status codes in the range 3000-3999 are reserved for use by
/// libraries, frameworks, and applications.  These status codes are
/// registered directly with IANA.  The interpretation of these codes
/// is undefined by this protocol.
///
/// 4000..4999 - Available for applications
/// Status codes in the range 4000-4999 are reserved for private use
/// and thus can't be registered.  Such codes can be used by prior
/// agreements between WebSocket applications.  The interpretation of
/// these codes is undefined by this protocol.
/// {@category Entity}
enum WebSocketStatusCodes implements Comparable<WebSocketStatusCodes> {
  /// Successful operation / regular socket shutdown.
  /// The connection successfully completed
  /// the purpose for which it was created.
  normalClosure(1000, 'NORMAL_CLOSURE'),

  /// Client is leaving.
  /// The endpoint is going away, either because of a server failure
  /// or because the browser is navigating away from the page
  /// that opened the connection.
  /// For example, a browser tab closing.
  goingAway(1001, 'GOING_AWAY'),

  /// The endpoint is terminating the connection due to a protocol error.
  protocolError(1002, 'PROTOCOL_ERROR'),

  /// The connection is being terminated because the endpoint received data
  /// of a type it cannot accept.
  /// For example, a text-only endpoint received binary data.
  unsupportedData(1003, 'UNSUPPORTED_DATA'),

  /// Reserved.
  /// A meaning might be defined in the future.
  @internal
  reserved(1004, 'RESERVED'),

  /// Reserved.
  /// Indicates that no status code was provided even though one was expected.
  @internal
  noStatusReceived(1005, 'NO_STATUS_RECEIVED'),

  /// Reserved.
  /// Indicates that a connection was closed abnormally
  /// when a status code is expected.
  @internal
  abnormalClosure(1006, 'ABNORMAL_CLOSURE'),

  /// The endpoint is terminating the connection because
  /// a message was received that contained inconsistent data.
  /// For example, non-UTF-8 data within a text message.
  invalidFramePayloadData(1007, 'UNSUPPORTED_PAYLOAD'),

  /// The endpoint is terminating the connection because it received
  /// a message that violates its policy.
  /// This is a generic status code,
  /// used when codes 1003 and 1009 are not suitable.
  policyViolation(1008, 'POLICY_VIOLATION'),

  /// The endpoint is terminating the connection because
  /// a data frame was received that is too large.
  messageTooBig(1009, 'TOO_LARGE'),

  /// The client is terminating the connection because
  /// it expected the server to negotiate one or more extension,
  /// but the server didn't.
  mandatoryExtension(1010, 'MANDATORY_EXTENSION'),

  /// The server is terminating the connection because it encountered
  /// an unexpected condition that prevented it from fulfilling the request.
  internalError(1011, 'INTERNAL_ERROR'),

  /// The server is terminating the connection because it is restarting.
  serviceRestart(1012, 'RESTART'),

  /// The server is terminating the connection due to a temporary condition.
  /// For example, it is overloaded and is casting off some of its clients.
  tryAgainLater(1013, 'TRY_AGAIN_LATER'),

  /// The server was acting as a gateway or proxy and received
  /// an invalid response from the upstream server.
  /// This is similar to 502 HTTP Status Code.
  badGateway(1014, 'BAD_GATEWAY'),

  /// Reserved.
  /// Indicates that the connection was closed due to a failure
  /// to perform a TLS handshake.
  /// For example, the server certificate can't be verified.
  tlsHandshake(1015, 'TLS_HANDSHAKE');

  /// WebSocket status codes.
  const WebSocketStatusCodes(this.code, this.codename);

  /// Returns the [WebSocketStatusCodes] by the given [code].
  static WebSocketStatusCodes? valueOf(int code) {
    for (final value in values) {
      if (value.code == code) return value;
    }
    return null;
  }

  /// Close code (uint16).
  final int code;

  /// Meaning / Codename
  final String codename;

  @override
  int compareTo(WebSocketStatusCodes other) => code.compareTo(other.code);
}

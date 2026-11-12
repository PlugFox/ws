/// RFC 6455 framing constants and close-code rules.
library;

/// Frame opcodes (RFC 6455 §5.2).
abstract final class Opcode {
  /// Continuation of a fragmented message.
  static const int continuation = 0x0;

  /// Text message (UTF-8).
  static const int text = 0x1;

  /// Binary message.
  static const int binary = 0x2;

  /// Close control frame.
  static const int close = 0x8;

  /// Ping control frame.
  static const int ping = 0x9;

  /// Pong control frame.
  static const int pong = 0xA;
}

/// Close codes used by the protocol layer itself (RFC 6455 §7.4.1).
// ignore: avoid_classes_with_only_static_members
abstract final class CloseCode {
  /// Normal closure.
  static const int normal = 1000;

  /// The endpoint is going away.
  static const int goingAway = 1001;

  /// Protocol error.
  static const int protocolError = 1002;

  /// Unsupported data.
  static const int unsupportedData = 1003;

  /// No status code was present. Never sent on the wire.
  static const int noStatus = 1005;

  /// The connection was closed without a close frame. Never sent on the
  /// wire.
  static const int abnormal = 1006;

  /// Text message was not valid UTF-8.
  static const int invalidPayload = 1007;

  /// Policy violation.
  static const int policyViolation = 1008;

  /// Message too big.
  static const int messageTooBig = 1009;

  /// Server encountered an unexpected condition.
  static const int internalError = 1011;

  /// Whether [code] may appear in a close frame on the wire.
  static bool isValidOnWire(int code) => switch (code) {
    >= 1000 && <= 1003 => true,
    >= 1007 && <= 1014 => true,
    >= 3000 && <= 4999 => true,
    _ => false,
  };
}

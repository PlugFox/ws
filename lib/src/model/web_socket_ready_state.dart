/// The [IWebSocketPlatformTransport.readyState] property
/// returns the current state of the WebSocket connection.
enum WebSocketReadyState {
  /// Socket has been created. The connection is not yet open.
  connecting(
    0,
    'CONNECTING',
    'Socket has been created. The connection is not yet open.',
  ),

  /// The connection is open and ready to communicate.
  open(
    1,
    'OPEN',
    'The connection is open and ready to communicate.',
  ),

  /// The connection is in the process of closing.
  closing(
    2,
    'CLOSING',
    'The connection is in the process of closing.',
  ),

  /// The connection is closed or couldn't be opened.
  closed(
    3,
    'CLOSED',
    'The connection is closed or couldn\'t be opened.',
  );

  /// Creates a const [WebSocketReadyState] with the specified [code],
  /// [name] and [description].
  const WebSocketReadyState(this.code, this.name, this.description);

  /// Creates a [WebSocketReadyState] from the specified [code].
  factory WebSocketReadyState.fromCode(int code) {
    switch (code) {
      case 0:
        return WebSocketReadyState.connecting;
      case 1:
        return WebSocketReadyState.open;
      case 2:
        return WebSocketReadyState.closing;
      case 3:
        return WebSocketReadyState.closed;
      default:
        throw ArgumentError.value(
          code,
          'code',
          'Invalid WebSocketReadyState code.',
        );
    }
  }

  /// The value of the [WebSocketReadyState] constant.
  final int code;

  /// Name of the [WebSocketReadyState] constant.
  final String name;

  /// Description of the [WebSocketReadyState] constant.
  final String description;

  /// Socket has been created. The connection is not yet open.
  bool get isConnecting => this == WebSocketReadyState.connecting;

  /// The connection is open and ready to communicate.
  bool get isOpen => this == WebSocketReadyState.open;

  /// The connection is in the process of closing.
  bool get isClosing => this == WebSocketReadyState.closing;

  /// The connection is closed or couldn't be opened.
  bool get isClosed => this == WebSocketReadyState.closed;

  @override
  String toString() => name;
}

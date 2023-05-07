/// WebSocket client state.
/// {@category Client}
/// {@category Entity}

/// Returns the current state of the connection.
/// 0	: CONNECTING - Socket has been created. The connection is not yet open.
/// 1	: OPEN       - The connection is open and ready to communicate.
/// 2	: CLOSING    - The connection is in the process of closing.
/// 3	: CLOSED     - The connection is closed or couldn't be opened.
//WebSocketReadyState get readyState;

/// The close code set when the WebSocket connection is closed.
/// If there is no close code available this property will be null.
//int? get closeCode;

/// The close reason set when the WebSocket connection is closed.
/// If there is no close reason available this property will be null.
//String? get closeReason;

/// Whether the stream controller is permanently closed.
///
/// The controller becomes closed by calling the [close] method.
///
/// If the controller is closed,
/// the "done" event might not have been delivered yet,
/// but it has been scheduled, and it is too late to add more events.
//bool get isClosed;

/// A future which is completed when the stream controller is done.
//Future<void> get done;
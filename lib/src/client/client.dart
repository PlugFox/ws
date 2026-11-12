import 'dart:async';
import 'dart:collection';
import 'dart:convert' show utf8;
import 'dart:typed_data';

import 'package:clock/clock.dart';
import 'package:ws/src/client/codec.dart';
import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/client/metrics.dart';
import 'package:ws/src/client/options.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/transport/transport.dart';
import 'package:ws/src/transport/transport_registry.dart';
import 'package:ws/src/util/backoff.dart';

/// A WebSocket client with reconnects, heartbeats and request/response
/// calls, on every platform.
///
/// ```dart
/// final client = WebSocketClient.json(
///   options: const WebSocketOptions(rpc: WebSocketJsonRpc()),
/// );
/// client.messages.listen(print);
/// await client.connect('wss://example.com/ws');
/// final reply = await client.rpc({'method': 'ping'});
/// await client.close();
/// ```
abstract interface class WebSocketClient<T>
    implements WebSocketClientInterface<T> {
  /// A client for binary messages. Text messages arrive as UTF-8 bytes.
  // ignore: prefer_constructors_over_static_methods
  static WebSocketClient<Uint8List> binary({
    WebSocketOptions<Uint8List>? options,
  }) => WebSocketClientImpl<Uint8List>(
    const BinaryCodec(),
    options ?? const WebSocketOptions(),
  );

  /// A client for JSON objects sent and received as text messages.
  // ignore: prefer_constructors_over_static_methods
  static WebSocketClient<Map<String, Object?>> json({
    WebSocketOptions<Map<String, Object?>>? options,
  }) => WebSocketClientImpl<Map<String, Object?>>(
    const JsonCodec(),
    options ?? const WebSocketOptions(),
  );

  /// A client for text messages.
  // ignore: prefer_constructors_over_static_methods
  static WebSocketClient<String> text({WebSocketOptions<String>? options}) =>
      WebSocketClientImpl<String>(
        const TextCodec(),
        options ?? const WebSocketOptions(),
      );

  /// The current state.
  WebSocketState get state;

  /// State changes. Synchronous broadcast stream; closed by [close].
  Stream<WebSocketState> get states;

  /// Incoming messages. Synchronous broadcast stream without backpressure;
  /// for the lowest overhead use `WebSocketOptions.onMessage`. RPC replies
  /// and application pongs are not delivered here.
  Stream<T> get messages;

  /// Counters and the last error.
  WebSocketMetrics get metrics;

  /// Connects to [url] (`ws://`, `wss://`; `http(s)://` is accepted) and
  /// keeps the connection up according to the reconnect policy.
  ///
  /// Completes when the connection first opens. Fails when the attempt
  /// fails in a way that is not retried, or when [disconnect] or [close]
  /// is called first. With unlimited reconnects it may wait indefinitely;
  /// add a timeout if needed. Calling it again with another URL switches
  /// to that URL.
  Future<void> connect(String url);

  /// Sends a message. Completes when the transport accepted it; while more
  /// than `sendHighWaterMark` bytes are queued, completes only after the
  /// queue drains. Fails with [WebSocketNotConnectedException] when not
  /// open: messages are never buffered across reconnects.
  @override
  Future<void> push(T data);

  /// Sends a request and waits for the matching reply. Needs
  /// `WebSocketOptions.rpc`. Fails with [WebSocketRpcException] on timeout,
  /// disconnect, close or an error reply; never retried automatically.
  @override
  Future<T> rpc(T data, {Duration? timeout});

  /// Sends a ping and completes when the pong arrives (round trip in
  /// [metrics]). Uses the application-level ping from
  /// `WebSocketHeartbeat` when configured, otherwise a protocol ping; in the
  /// browser without an application ping it throws
  /// [WebSocketUnsupportedException].
  Future<void> ping({Duration? timeout});

  /// Drops the current connection or the reconnect wait and connects again
  /// now (for example when the app returns to the foreground). Needs an
  /// earlier [connect].
  Future<void> reconnect();

  /// Closes the connection and stops reconnecting. The client can
  /// [connect] again later. Waits for the closing handshake at most
  /// `disconnectTimeout`.
  Future<void> disconnect([int? code = 1000, String? reason]);

  /// Disconnects and releases everything. Terminal and idempotent.
  Future<void> close([int? code = 1000, String? reason]);
}

/// The implementation shared by every transport.
final class WebSocketClientImpl<T> implements WebSocketClient<T> {
  /// Creates a client with [_codec] and [_options].
  WebSocketClientImpl(this._codec, this._options)
    : _transport = _options.transport ?? WebSocketTransports.current;

  final MessageCodec<T> _codec;
  final WebSocketOptions<T> _options;
  final WebSocketTransport _transport;

  WebSocketState _state = const WebSocketState.disconnected();
  final StreamController<WebSocketState> _states =
      StreamController<WebSocketState>.broadcast(sync: true);
  final StreamController<T> _messages = StreamController<T>.broadcast(
    sync: true,
  );

  Uri? _url;
  bool _wanted = false;
  bool _closed = false;
  int _failures = 0;
  _Session<T>? _session;
  Timer? _reconnectTimer;
  final List<Completer<void>> _connectWaiters = <Completer<void>>[];

  // RPC.
  int _seq = 0;
  final Map<Object, _PendingCall<T>> _calls = <Object, _PendingCall<T>>{};
  Timer? _rpcTimer;

  // Metrics.
  int _sent = 0;
  int _received = 0;
  int _bytesSent = 0;
  int _bytesReceived = 0;
  int _connects = 0;
  int _reconnectAttempts = 0;
  int _decodeErrors = 0;
  int _droppedReplies = 0;
  Duration? _lastRoundTrip;
  WebSocketException? _lastError;

  @override
  WebSocketState get state => _state;

  @override
  Stream<WebSocketState> get states => _states.stream;

  @override
  Stream<T> get messages => _messages.stream;

  @override
  WebSocketMetrics get metrics => WebSocketMetrics(
    transport: _transport.name,
    messagesSent: _sent,
    messagesReceived: _received,
    bytesSent: _bytesSent,
    bytesReceived: _bytesReceived,
    connects: _connects,
    reconnectAttempts: _reconnectAttempts,
    decodeErrors: _decodeErrors,
    droppedReplies: _droppedReplies,
    pendingRpc: _calls.length,
    bufferedAmount: _session?.connection?.bufferedAmount ?? 0,
    lastRoundTrip: _lastRoundTrip,
    lastError: _lastError,
    openedAt: _session?.openedAt,
  );

  // --- state ---------------------------------------------------------------

  final Queue<WebSocketState> _stateEvents = Queue<WebSocketState>();
  bool _emittingStates = false;

  /// Updates [state] immediately and delivers the change in order.
  ///
  /// Listeners may call `connect`, `disconnect` or `close` from inside a
  /// state callback; those changes are queued and delivered after the
  /// current one instead of re-entering the stream (which would throw).
  void _setState(WebSocketState next) {
    if (_state.isClosed) return;
    _state = next;
    _stateEvents.add(next);
    if (_emittingStates) return;
    _emittingStates = true;
    try {
      while (_stateEvents.isNotEmpty) {
        final event = _stateEvents.removeFirst();
        _guard(() => _options.onStateChange?.call(event));
        if (!_states.isClosed) _states.add(event);
      }
    } finally {
      _emittingStates = false;
    }
  }

  void _guard(void Function() fn) {
    try {
      fn();
    } on Object catch (e, s) {
      _reportError(e, s);
    }
  }

  void _reportError(Object error, StackTrace stackTrace) {
    if (error is WebSocketException) _lastError = error;
    final onError = _options.onError;
    if (onError == null) return;
    try {
      onError(error, stackTrace);
    } on Object catch (e, s) {
      Zone.current.handleUncaughtError(e, s);
    }
  }

  // --- connect ---------------------------------------------------------------

  static Uri _parseUrl(String url) {
    final uri = Uri.parse(url);
    return switch (uri.scheme) {
      'ws' || 'wss' => uri,
      'http' => uri.replace(scheme: 'ws'),
      'https' => uri.replace(scheme: 'wss'),
      _ => throw ArgumentError.value(
        url,
        'url',
        'Expected a ws:// or wss:// URL',
      ),
    };
  }

  Future<void> _waitForOpen() {
    if (_state.isOpen) return Future<void>.value();
    final waiter = Completer<void>();
    _connectWaiters.add(waiter);
    // A fire-and-forget connect() must not surface as an unhandled error
    // when it later fails; awaiting callers still get the error, and the
    // failure is also visible in `state` and `onError` (DECISIONS.md D21).
    return waiter.future..ignore();
  }

  void _completeWaiters([WebSocketException? error]) {
    if (_connectWaiters.isEmpty) return;
    final waiters = List<Completer<void>>.of(_connectWaiters);
    _connectWaiters.clear();
    for (final w in waiters) {
      error == null ? w.complete() : w.completeError(error);
    }
  }

  @override
  Future<void> connect(String url) {
    if (_closed) {
      return Future<void>.error(StateError('The client is closed'));
    }
    final uri = _parseUrl(url);
    if (_wanted &&
        _url == uri &&
        (_session != null || _reconnectTimer != null)) {
      return _waitForOpen();
    }
    _url = uri;
    _wanted = true;
    _failures = 0;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _dropSession(1000, 'Switching URL');
    _start();
    return _waitForOpen();
  }

  @override
  Future<void> reconnect() {
    if (_closed) {
      return Future<void>.error(StateError('The client is closed'));
    }
    if (!_wanted || _url == null) {
      return Future<void>.error(
        const WebSocketNotConnectedException('connect() was not called'),
      );
    }
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _dropSession(1001, 'Reconnecting');
    _start();
    return _waitForOpen();
  }

  /// Detaches the current session; its events are ignored from now on.
  void _dropSession(int code, String reason) {
    final session = _session;
    if (session == null) return;
    _session = null;
    session.dispose();
    _failCalls(WebSocketRpcFailure.disconnected);
    final connection = session.connection;
    if (connection == null) return;
    if (session.transportOpen) {
      connection.close(code, reason);
      // Do not wait: abort if the peer does not finish the handshake.
      Timer(_options.disconnectTimeout, connection.abort);
    } else {
      connection.abort();
    }
  }

  void _start() {
    final url = _url!;
    final attempt = _failures;
    final session = _Session<T>(this);
    _session = session;
    _setState(WebSocketState.connecting(url: url, attempt: attempt));
    Future<void>.microtask(() async {
      WebSocketRequest request;
      try {
        request =
            await (_options.request?.call(url, attempt) ??
                WebSocketRequest(url, headers: _options.vm.headers));
      } on Object catch (e, s) {
        if (!identical(_session, session)) return;
        _reportError(e, s);
        _onSessionClosed(
          session,
          null,
          '',
          WebSocketConnectionException('The request provider failed', cause: e),
        );
        return;
      }
      if (!identical(_session, session)) return;
      session
        ..connectTimer = Timer(_options.connectTimeout, () {
          session.fail(const WebSocketTimeoutException('Connect timed out'));
        })
        ..connection = _transport.connect(
          TransportRequest(
            url: request.url,
            headers: request.headers,
            protocols: _options.protocols,
            maxMessageSize: _options.maxMessageSize,
            textMode: _codec.textMode,
            tls: _options.vm.tls,
          ),
          session,
        );
    });
  }

  Future<void> _onTransportOpen(_Session<T> session, String? protocol) async {
    session
      ..connectTimer?.cancel()
      ..transportOpen = true;
    final onOpen = _options.onOpen;
    if (onOpen != null) {
      try {
        await onOpen(this);
      } on Object catch (e, s) {
        if (!identical(_session, session)) return;
        _reportError(e, s);
        session.fail(
          WebSocketConnectionException('onOpen callback failed', cause: e),
        );
        return;
      }
      if (!identical(_session, session)) return;
    }
    session.openedAt = clock.now();
    _connects++;
    final stableAfter = _options.reconnect.stableAfter;
    session.stableTimer = Timer(stableAfter, () => _failures = 0);
    _setState(WebSocketState.open(url: _url!, protocol: protocol));
    _completeWaiters();
    session.startHeartbeat(
      _options.heartbeat,
      supportsPing: _transport.supportsPing,
    );
  }

  void _onSessionClosed(
    _Session<T> session,
    int? code,
    String reason,
    WebSocketException? error,
  ) {
    if (!identical(_session, session)) return;
    _session = null;
    session.dispose();
    _failCalls(WebSocketRpcFailure.disconnected);

    final cause =
        session.failure ??
        error ??
        WebSocketClosedException(
          'Connection closed (${code ?? 'no status'})',
          code: code,
          reason: reason,
          byServer: !session.closingByUs,
        );

    if (session.byUser || !_wanted) {
      _setState(
        WebSocketState.disconnected(closeCode: code, closeReason: reason),
      );
      session.closed.complete();
      return;
    }
    session.closed.complete();

    final attempt = _failures + 1;
    if (!_shouldReconnect(cause, attempt)) {
      _wanted = false;
      _lastError = cause;
      _setState(
        WebSocketState.disconnected(
          closeCode: code,
          closeReason: reason,
          error: cause,
        ),
      );
      _completeWaiters(cause);
      return;
    }
    _reportError(cause, StackTrace.current);
    _failures = attempt;
    _reconnectAttempts++;
    final reconnect = _options.reconnect;
    var delay = backoffDelay(
      attempt - 1,
      reconnect.minDelay,
      reconnect.maxDelay,
    );
    if (cause case WebSocketClosedException(code: 1013)) {
      delay = reconnect.maxDelay; // Try Again Later
    }
    _setState(
      WebSocketState.reconnecting(attempt: attempt, delay: delay, cause: cause),
    );
    _reconnectTimer = Timer(delay, () {
      _reconnectTimer = null;
      if (_wanted && !_closed) _start();
    });
  }

  bool _shouldReconnect(WebSocketException cause, int attempt) {
    final policy = _options.reconnect;
    if (!policy.enabled) return false;
    if (policy.maxAttempts case final max? when attempt > max) return false;
    if (policy.shouldReconnect case final decide?) {
      try {
        return decide(cause, attempt);
      } on Object catch (e, s) {
        _reportError(e, s);
        return false;
      }
    }
    return switch (cause) {
      WebSocketHandshakeException(:final statusCode?)
          when statusCode >= 400 && statusCode < 500 =>
        statusCode == 408 || statusCode == 425 || statusCode == 429,
      WebSocketClosedException(:final code?) =>
        !policy.terminalCloseCodes.contains(code),
      _ => true,
    };
  }

  // --- disconnect / close ------------------------------------------------------

  /// Browsers accept only 1000 and 3000–4999 and a reason of at most 123
  /// UTF-8 bytes; the same rule applies everywhere so code behaves alike.
  static void _checkClose(int? code, String? reason) {
    if (code != null && code != 1000 && (code < 3000 || code > 4999)) {
      throw ArgumentError.value(
        code,
        'code',
        'Must be 1000 or in the range 3000–4999',
      );
    }
    if (reason != null && utf8.encode(reason).length > 123) {
      throw ArgumentError.value(
        reason,
        'reason',
        'Must be at most 123 bytes in UTF-8',
      );
    }
  }

  @override
  Future<void> disconnect([int? code = 1000, String? reason]) {
    _checkClose(code, reason);
    _wanted = false;
    _reconnectTimer?.cancel();
    _reconnectTimer = null;
    _completeWaiters(
      const WebSocketNotConnectedException('disconnect() was called'),
    );
    final session = _session;
    if (session == null) {
      if (_state is! WebSocketState$Disconnected) {
        _setState(const WebSocketState.disconnected());
      }
      return Future<void>.value();
    }
    if (session.byUser) return session.closed.future;
    session
      ..byUser = true
      ..closingByUs = true;
    final connection = session.connection;
    if (connection == null || !session.transportOpen) {
      // Still connecting: nothing to close politely.
      if (connection == null) {
        _onSessionClosed(session, null, '', null);
      } else {
        connection.abort();
      }
      return session.closed.future;
    }
    _setState(WebSocketState.disconnecting(code: code, reason: reason));
    session.stopHeartbeat();
    connection.close(code, reason);
    session.closeTimer = Timer(_options.disconnectTimeout, connection.abort);
    return session.closed.future;
  }

  @override
  Future<void> close([int? code = 1000, String? reason]) async {
    if (_closed) return;
    _closed = true;
    await disconnect(code, reason);
    _failCalls(WebSocketRpcFailure.closed);
    _rpcTimer?.cancel();
    _setState(const WebSocketState.closed());
    await _states.close();
    await _messages.close();
  }

  // --- send ------------------------------------------------------------------

  TransportConnection? get _openConnection {
    final session = _session;
    if (session == null || !session.transportOpen || session.byUser) {
      return null;
    }
    return session.connection;
  }

  @override
  Future<void> push(T data) {
    final connection = _openConnection;
    if (connection == null) {
      return Future<void>.error(
        _closed
            ? const WebSocketNotConnectedException('The client is closed')
            : const WebSocketNotConnectedException('Not connected'),
      );
    }
    final before = connection.bufferedAmount;
    try {
      _codec.send(connection, data);
    } on WebSocketException catch (e, s) {
      return Future<void>.error(e, s);
    }
    _sent++;
    final buffered = connection.bufferedAmount;
    if (buffered > before) _bytesSent += buffered - before;
    if (buffered <= _options.sendHighWaterMark) return Future<void>.value();
    final drained = _session!.waitForDrain();
    connection.requestDrain();
    return drained;
  }

  @override
  Future<T> rpc(T data, {Duration? timeout}) {
    final handler = _options.rpc;
    assert(handler != null, 'rpc() needs WebSocketOptions.rpc');
    if (handler == null) {
      return Future<T>.error(StateError('rpc() needs WebSocketOptions.rpc'));
    }
    if (_openConnection == null) {
      return Future<T>.error(
        const WebSocketNotConnectedException('Not connected'),
      );
    }
    final Object id;
    final T message;
    try {
      (id, message) = handler.encode(++_seq, data);
    } on Object catch (e, s) {
      return Future<T>.error(e, s);
    }
    final call = _PendingCall<T>(
      id,
      clock.now().add(timeout ?? _options.rpcTimeout),
    );
    _calls[id] = call;
    _scheduleRpcTimer();
    push(message).catchError((Object e, StackTrace s) {
      if (identical(_calls[id], call)) {
        _calls.remove(id);
        call.completer.completeError(
          WebSocketRpcException(
            'Sending the request failed',
            failure: WebSocketRpcFailure.disconnected,
            id: id,
            cause: e,
          ),
          s,
        );
      }
    });
    return call.completer.future;
  }

  void _scheduleRpcTimer() {
    if (_calls.isEmpty) {
      _rpcTimer?.cancel();
      _rpcTimer = null;
      return;
    }
    var next = _calls.values.first.deadline;
    for (final call in _calls.values) {
      if (call.deadline.isBefore(next)) next = call.deadline;
    }
    if (_rpcTimer != null && _rpcDeadline == next) return;
    _rpcTimer?.cancel();
    _rpcDeadline = next;
    final wait = next.difference(clock.now());
    _rpcTimer = Timer(wait.isNegative ? Duration.zero : wait, _expireCalls);
  }

  DateTime? _rpcDeadline;

  void _expireCalls() {
    _rpcTimer = null;
    _rpcDeadline = null;
    final now = clock.now();
    final expired = <Object>[
      for (final MapEntry(:key, :value) in _calls.entries)
        if (!value.deadline.isAfter(now)) key,
    ];
    for (final id in expired) {
      _calls
          .remove(id)!
          .completer
          .completeError(
            WebSocketRpcException(
              'No reply within the timeout',
              failure: WebSocketRpcFailure.timeout,
              id: id,
            ),
          );
    }
    _scheduleRpcTimer();
  }

  void _failCalls(WebSocketRpcFailure failure) {
    if (_calls.isEmpty) return;
    final calls = List<_PendingCall<T>>.of(_calls.values);
    _calls.clear();
    _scheduleRpcTimer();
    for (final call in calls) {
      call.completer.completeError(
        WebSocketRpcException(
          failure == WebSocketRpcFailure.closed
              ? 'The client was closed'
              : 'The connection was lost before the reply',
          failure: failure,
          id: call.id,
        ),
      );
    }
  }

  @override
  Future<void> ping({Duration? timeout}) {
    final session = _session;
    final connection = _openConnection;
    if (session == null || connection == null) {
      return Future<void>.error(
        const WebSocketNotConnectedException('Not connected'),
      );
    }
    return session.ping(
      timeout ?? _options.heartbeat?.timeout ?? const Duration(seconds: 10),
    );
  }

  // --- receive ---------------------------------------------------------------

  void _onMessage(_Session<T> session, T message, int bytes) {
    session.touch();
    _received++;
    _bytesReceived += bytes;
    if (session.openedAt != null) _failures = 0;

    if (_options.heartbeat?.isPong case final isPong?) {
      var pong = false;
      try {
        pong = isPong(message);
      } on Object catch (e, s) {
        _reportError(e, s);
      }
      if (pong) return session.pongReceived();
    }

    if (_options.rpc case final handler?) {
      Object? id;
      try {
        id = handler.match(message);
      } on Object catch (e, s) {
        _reportError(e, s);
      }
      if (id != null) {
        final call = _calls.remove(id);
        if (call == null) {
          _droppedReplies++;
          return;
        }
        _scheduleRpcTimer();
        try {
          call.completer.complete(handler.decode(message));
        } on WebSocketRpcException catch (e, s) {
          call.completer.completeError(e, s);
        } on Object catch (e, s) {
          call.completer.completeError(
            WebSocketRpcException(
              'The reply could not be decoded',
              failure: WebSocketRpcFailure.remote,
              id: id,
              cause: e,
            ),
            s,
          );
        }
        return;
      }
    }

    if (_options.onMessage case final onMessage?) {
      _guard(() => onMessage(message));
    }
    if (_messages.hasListener) _messages.add(message);
  }

  void _onDecodeError(_Session<T> session, Object error, StackTrace stack) {
    if (error is InvalidUtf8) {
      session.connection?.close(1007, 'Invalid UTF-8');
      session.failure ??= const WebSocketClosedException(
        'Received invalid UTF-8',
        code: 1007,
        byServer: false,
      );
      return;
    }
    _decodeErrors++;
    _reportError(
      error is WebSocketException
          ? error
          : WebSocketFormatException(
              'Could not decode a message',
              cause: error,
            ),
      stack,
    );
  }
}

final class _PendingCall<T> {
  _PendingCall(this.id, this.deadline);

  final Object id;
  final DateTime deadline;
  final Completer<T> completer = Completer<T>();
}

/// One connection attempt and, if it opens, the connection. Stale sessions
/// (replaced by a newer one) are ignored by the client.
final class _Session<T> implements TransportListener {
  _Session(this._client);

  final WebSocketClientImpl<T> _client;

  TransportConnection? connection;
  bool transportOpen = false;
  bool byUser = false;
  bool closingByUs = false;
  WebSocketException? failure;
  DateTime? openedAt;
  final Completer<void> closed = Completer<void>();

  Timer? connectTimer;
  Timer? closeTimer;
  Timer? stableTimer;
  Timer? _heartbeatTimer;
  final Stopwatch _sinceInbound = clock.stopwatch()..start();

  final Queue<Completer<void>> _drainWaiters = Queue<Completer<void>>();
  final List<Completer<void>> _pings = <Completer<void>>[];
  final Stopwatch _pingClock = clock.stopwatch();

  bool get _current => identical(_client._session, this);

  void touch() => _sinceInbound
    ..reset()
    ..start();

  /// Drops the connection; `onClosed` reports [cause].
  void fail(WebSocketException cause) {
    failure ??= cause;
    closingByUs = true;
    final c = connection;
    if (c == null) {
      _client._onSessionClosed(this, null, '', cause);
    } else {
      c.abort();
    }
  }

  Future<void> waitForDrain() {
    final waiter = Completer<void>();
    _drainWaiters.add(waiter);
    return waiter.future;
  }

  void dispose() {
    connectTimer?.cancel();
    closeTimer?.cancel();
    stableTimer?.cancel();
    stopHeartbeat();
    const error = WebSocketNotConnectedException('The connection was lost');
    while (_drainWaiters.isNotEmpty) {
      _drainWaiters.removeFirst().completeError(error);
    }
    for (final p in _pings) {
      p.completeError(error);
    }
    _pings.clear();
  }

  // --- heartbeat ---

  WebSocketHeartbeat<T>? _heartbeat;

  void startHeartbeat(
    WebSocketHeartbeat<T>? heartbeat, {
    required bool supportsPing,
  }) {
    if (heartbeat == null) return;
    // Without pings the watchdog would kill healthy idle connections.
    if (heartbeat.ping == null && !supportsPing) return;
    _heartbeat = heartbeat;
    touch();
    _scheduleHeartbeat(heartbeat.interval);
  }

  void stopHeartbeat() {
    _heartbeatTimer?.cancel();
    _heartbeatTimer = null;
    _heartbeat = null;
  }

  void _scheduleHeartbeat(Duration wait) {
    _heartbeatTimer?.cancel();
    _heartbeatTimer = Timer(wait, _onHeartbeat);
  }

  void _onHeartbeat() {
    final heartbeat = _heartbeat;
    if (heartbeat == null || !_current) return;
    final idle = _sinceInbound.elapsed;
    if (idle < heartbeat.interval) {
      return _scheduleHeartbeat(heartbeat.interval - idle);
    }
    // Idle long enough (or the timer fired late after a suspension): probe
    // first, and only give up if the probe gets no answer.
    final mark = _sinceInbound.elapsedMicroseconds;
    _sendPing();
    _heartbeatTimer = Timer(heartbeat.timeout, () {
      if (!_current || _heartbeat == null) return;
      if (_sinceInbound.elapsedMicroseconds >= mark) {
        fail(const WebSocketTimeoutException('Heartbeat timed out'));
      } else {
        _scheduleHeartbeat(heartbeat.interval);
      }
    });
  }

  void _sendPing() {
    final c = connection;
    if (c == null) return;
    _pingClock
      ..reset()
      ..start();
    try {
      if (_client._options.heartbeat?.ping case final ping?) {
        _client._codec.send(c, ping());
      } else {
        c.ping();
      }
    } on Object catch (e, s) {
      _client._reportError(e, s);
    }
  }

  Future<void> ping(Duration timeout) {
    final c = connection;
    if (c == null) {
      return Future<void>.error(
        const WebSocketNotConnectedException('Not connected'),
      );
    }
    final appPing = _client._options.heartbeat?.ping;
    if (appPing == null && !_client._transport.supportsPing) {
      return Future<void>.error(
        const WebSocketUnsupportedException(
          'Protocol pings are not available on this platform; '
          'configure WebSocketHeartbeat.ping and isPong',
        ),
      );
    }
    final waiter = Completer<void>();
    _pings.add(waiter);
    if (_pings.length == 1) _sendPing();
    return waiter.future.timeout(
      timeout,
      onTimeout: () {
        _pings.remove(waiter);
        throw const WebSocketTimeoutException('No pong within the timeout');
      },
    );
  }

  void pongReceived() {
    touch();
    if (_pingClock.isRunning) {
      _client._lastRoundTrip = _pingClock.elapsed;
      _pingClock.stop();
    }
    if (_pings.isEmpty) return;
    final waiters = List<Completer<void>>.of(_pings);
    _pings.clear();
    for (final w in waiters) {
      w.complete();
    }
  }

  // --- TransportListener ---

  @override
  void onOpen(String? protocol) {
    if (!_current) return;
    _client._onTransportOpen(this, protocol);
  }

  @override
  void onTextBytes(Uint8List bytes) {
    if (!_current) return;
    final T message;
    try {
      message = _client._codec.fromTextBytes(bytes);
    } on Object catch (e, s) {
      return _client._onDecodeError(this, e, s);
    }
    _client._onMessage(this, message, bytes.length);
  }

  @override
  void onTextString(String text) {
    if (!_current) return;
    final T message;
    try {
      message = _client._codec.fromTextString(text);
    } on Object catch (e, s) {
      return _client._onDecodeError(this, e, s);
    }
    _client._onMessage(this, message, text.length);
  }

  @override
  void onJson(Object? value) {
    if (!_current) return;
    final T message;
    try {
      message = _client._codec.fromJson(value);
    } on Object catch (e, s) {
      return _client._onDecodeError(this, e, s);
    }
    _client._onMessage(this, message, 0);
  }

  @override
  void onBinary(Uint8List bytes) {
    if (!_current) return;
    final T message;
    try {
      message = _client._codec.fromBinary(bytes);
    } on Object catch (e, s) {
      return _client._onDecodeError(this, e, s);
    }
    _client._onMessage(this, message, bytes.length);
  }

  @override
  void onPong() {
    if (_current) pongReceived();
  }

  @override
  void onDrain() {
    if (!_current) return;
    while (_drainWaiters.isNotEmpty) {
      _drainWaiters.removeFirst().complete();
    }
  }

  @override
  void onClosed(int? code, String reason, WebSocketException? error) =>
      _client._onSessionClosed(this, code, reason, error);
}

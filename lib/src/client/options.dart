import 'dart:async';
import 'dart:convert' show base64, utf8;

import 'package:meta/meta.dart';
import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/client/rpc.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/transport/transport.dart';

/// The URL and headers for one connection attempt. Returned by
/// [WebSocketOptions.request] so tokens can be refreshed on every reconnect.
@immutable
final class WebSocketRequest {
  /// Creates a request.
  const WebSocketRequest(this.url, {this.headers = const <String, String>{}});

  /// The URL to connect to.
  final Uri url;

  /// Handshake headers. Ignored in the browser, which cannot send them.
  final Map<String, String> headers;
}

/// Reconnect policy. Delays use exponential backoff with full jitter.
@immutable
final class WebSocketReconnect {
  /// Reconnects forever with delays between [minDelay] and [maxDelay].
  const WebSocketReconnect({
    this.minDelay = const Duration(milliseconds: 500),
    this.maxDelay = const Duration(seconds: 15),
    this.maxAttempts,
    this.stableAfter = const Duration(seconds: 5),
    this.terminalCloseCodes = defaultTerminalCloseCodes,
    this.shouldReconnect,
  }) : enabled = true;

  /// Never reconnects automatically.
  const WebSocketReconnect.disabled()
    : enabled = false,
      minDelay = Duration.zero,
      maxDelay = Duration.zero,
      maxAttempts = 0,
      stableAfter = Duration.zero,
      terminalCloseCodes = defaultTerminalCloseCodes,
      shouldReconnect = null;

  /// Close codes after which the client does not reconnect: the server said
  /// that repeating the same thing would fail the same way.
  static const Set<int> defaultTerminalCloseCodes = <int>{
    1002, // protocol error
    1003, // unsupported data
    1007, // invalid payload
    1008, // policy violation
    1009, // message too big
    1010, // mandatory extension
  };

  /// Whether automatic reconnects are on.
  final bool enabled;

  /// Shortest delay before a reconnect.
  final Duration minDelay;

  /// Longest delay before a reconnect.
  final Duration maxDelay;

  /// Stop after this many failed attempts in a row; `null` = never stop.
  final int? maxAttempts;

  /// A connection must stay open this long (or receive a message) before
  /// the backoff starts again from [minDelay]. Protects servers that accept
  /// and immediately close from being hammered.
  final Duration stableAfter;

  /// See [defaultTerminalCloseCodes].
  final Set<int> terminalCloseCodes;

  /// Overrides the decision for every failure. [attempt] is the number of
  /// the next attempt. Return `false` to stop reconnecting.
  final bool Function(WebSocketException cause, int attempt)? shouldReconnect;
}

/// Keep-alive and dead-connection detection.
///
/// Native and pure-Dart transports send protocol pings. Browsers do not let
/// pages send pings or see pongs; there, the heartbeat only works with an
/// application-level [ping] message and [isPong] matcher.
@immutable
final class WebSocketHeartbeat<T> {
  /// Sends a ping after [interval] without incoming traffic, and drops the
  /// connection (then reconnects) if nothing arrives within [timeout].
  const WebSocketHeartbeat({
    this.interval = const Duration(seconds: 30),
    this.timeout = const Duration(seconds: 10),
    this.ping,
    this.isPong,
  });

  /// Idle time before a ping.
  final Duration interval;

  /// How long to wait for any incoming traffic after a ping.
  final Duration timeout;

  /// Application-level ping message. When set, it is used instead of a
  /// protocol ping on every platform.
  final T Function()? ping;

  /// Recognizes application-level pongs, which are then not delivered as
  /// messages.
  final bool Function(T message)? isPong;
}

/// TLS settings for native and pure-Dart transports, as plain data.
@immutable
final class WebSocketTls {
  /// Creates TLS settings.
  const WebSocketTls({this.trustedRootsPem, this.allowInsecure = false});

  /// Extra trusted root certificates (PEM). The platform trust store is
  /// still used.
  final String? trustedRootsPem;

  /// Accept any certificate. **Development only**; never the default.
  final bool allowInsecure;
}

/// An HTTP proxy reached with `CONNECT` (RFC 9110 §9.3.6), for native
/// platforms. The tunnel carries `ws://` and `wss://` alike; TLS runs end to
/// end through it.
@immutable
final class WebSocketProxy {
  /// Creates proxy settings.
  const WebSocketProxy({
    required this.host,
    required this.port,
    this.username,
    this.password,
  });

  /// Parses `http://[user:password@]host[:port]` (port defaults to 80) or
  /// `host:port`.
  factory WebSocketProxy.parse(String proxy) {
    final uri = Uri.parse(proxy.contains('://') ? proxy : 'http://$proxy');
    if (uri.scheme != 'http' || uri.host.isEmpty) {
      throw FormatException(
        'Expected http://[user:password@]host[:port]',
        proxy,
      );
    }
    final (username, password) = switch (uri.userInfo) {
      '' => (null, null),
      final info when info.contains(':') => (
        Uri.decodeComponent(info.substring(0, info.indexOf(':'))),
        Uri.decodeComponent(info.substring(info.indexOf(':') + 1)),
      ),
      final info => (Uri.decodeComponent(info), ''),
    };
    return WebSocketProxy(
      host: uri.host,
      port: uri.hasPort ? uri.port : 80,
      username: username,
      password: password,
    );
  }

  /// Proxy host name or address.
  final String host;

  /// Proxy port.
  final int port;

  /// User for `Proxy-Authorization: Basic`.
  final String? username;

  /// Password for `Proxy-Authorization: Basic`.
  final String? password;

  /// The `Proxy-Authorization` header value, if credentials are set.
  String? get authorization => username == null
      ? null
      : 'Basic ${base64.encode(utf8.encode('$username:${password ?? ''}'))}';

  @override
  String toString() => 'WebSocketProxy($host:$port)';
}

/// Options that only apply on native platforms (VM, Flutter mobile and
/// desktop). Plain data: compiles everywhere, ignored on the web.
@immutable
final class WebSocketVmOptions {
  /// Creates VM options.
  const WebSocketVmOptions({
    this.headers = const <String, String>{},
    this.tls = const WebSocketTls(),
    this.proxy,
  });

  /// Handshake headers (e.g. `Authorization`).
  final Map<String, String> headers;

  /// TLS settings.
  final WebSocketTls tls;

  /// HTTP `CONNECT` proxy; `null` connects directly. System proxy
  /// settings are not read (DECISIONS.md D36).
  final WebSocketProxy? proxy;
}

/// Options that only apply in the browser. Plain data: compiles everywhere,
/// ignored on native platforms.
@immutable
final class WebSocketJsOptions {
  /// Creates browser options.
  const WebSocketJsOptions();
}

/// Options of a `WebSocketClient`. The common part works on every
/// platform; [vm] and [js] hold the platform-specific parts.
@immutable
final class WebSocketOptions<T> {
  /// Creates options.
  const WebSocketOptions({
    this.protocols = const <String>[],
    this.connectTimeout = const Duration(seconds: 10),
    this.disconnectTimeout = const Duration(seconds: 3),
    this.reconnect = const WebSocketReconnect(),
    this.heartbeat,
    this.rpc,
    this.rpcTimeout = const Duration(seconds: 30),
    this.maxMessageSize = 64 << 20,
    this.sendHighWaterMark = 1 << 20,
    this.sendBufferLimit = 16 << 20,
    this.request,
    this.onOpen,
    this.onMessage,
    this.onStateChange,
    this.onError,
    this.transport,
    this.vm = const WebSocketVmOptions(),
    this.js = const WebSocketJsOptions(),
  });

  /// Subprotocols offered to the server.
  final List<String> protocols;

  /// Limit for one connection attempt (TCP, TLS and handshake).
  final Duration connectTimeout;

  /// How long `disconnect()` waits for the closing handshake before it
  /// drops the connection.
  final Duration disconnectTimeout;

  /// Reconnect policy.
  final WebSocketReconnect reconnect;

  /// Heartbeat; `null` disables it.
  final WebSocketHeartbeat<T>? heartbeat;

  /// Request/response correlation for `rpc()`. Without it `rpc()` throws.
  final WebSocketRpc<T>? rpc;

  /// Default timeout of `rpc()`.
  final Duration rpcTimeout;

  /// Largest incoming message, in bytes. Larger messages close the
  /// connection with 1009.
  final int maxMessageSize;

  /// When more than this many bytes wait to be sent, `push()` returns a
  /// future that completes only after the queue drains (backpressure).
  final int sendHighWaterMark;

  /// Hard limit on bytes waiting to be sent. While more than this is
  /// queued, `push()` fails with [WebSocketBufferFullException] instead of
  /// queueing (callers that ignore the futures of `push()` cannot grow
  /// memory without bound). The queue may exceed the limit by one message.
  final int sendBufferLimit;

  /// Called before every connection attempt with the URL given to
  /// `connect()`; returns the URL and headers to use. Use it to refresh
  /// tokens.
  final FutureOr<WebSocketRequest> Function(Uri url, int attempt)? request;

  /// Awaited after the transport opened and before the state becomes open.
  /// Messages can already be pushed here (authenticate, resubscribe). If it
  /// throws, the connection is dropped and the failure goes through the
  /// reconnect policy.
  final FutureOr<void> Function(WebSocketClientInterface<T> client)? onOpen;

  /// Called for every incoming message, before the `messages` stream.
  final void Function(T message)? onMessage;

  /// Called on every state change, before the `states` stream.
  final void Function(WebSocketState state)? onStateChange;

  /// Called for errors that do not close the client: decode failures,
  /// throwing callbacks, connection failures that will be retried.
  final void Function(Object error, StackTrace stackTrace)? onError;

  /// Overrides the transport (tests, a native transport, a fake).
  final WebSocketTransport? transport;

  /// Native-only options.
  final WebSocketVmOptions vm;

  /// Browser-only options.
  final WebSocketJsOptions js;
}

/// The part of the client available inside [WebSocketOptions.onOpen].
abstract interface class WebSocketClientInterface<T> {
  /// Sends a message.
  Future<void> push(T data);

  /// Sends a request and waits for its reply.
  Future<T> rpc(T data, {Duration? timeout});
}

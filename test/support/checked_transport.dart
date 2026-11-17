// A transport decorator that checks the TransportListener contract
// (transport.dart; TEST_PLAN T28) on every connection the suites open:
// callbacks never synchronous from a connection method or `connect`,
// `onOpen` at most once and before any message, `onDrain` only when
// requested, exactly one `onClosed`, always last. No dart:io here: the
// browser suite uses it too.
import 'dart:async';
import 'dart:typed_data';

import 'package:test/test.dart';
import 'package:ws/transport.dart';
import 'package:ws/ws.dart';

/// Wraps [inner] and records every contract violation.
final class CheckedTransport implements WebSocketTransport {
  CheckedTransport(this.inner);

  /// The transport under test.
  final WebSocketTransport inner;

  /// Violations so far, one line each.
  final List<String> violations = [];

  final List<_CheckedListener> _listeners = [];

  @override
  String get name => inner.name;

  @override
  bool get supportsPing => inner.supportsPing;

  @override
  bool get decodesJson => inner.decodesJson;

  @override
  TransportConnection connect(
    TransportRequest request,
    TransportListener listener,
  ) {
    final checked = _CheckedListener(this, listener, '${request.url}');
    _listeners.add(checked);
    checked._depth++;
    try {
      return checked._connection = _CheckedConnection(
        inner.connect(request, checked),
        checked,
      );
    } on Object {
      // No connection exists, so no onClosed is owed.
      _listeners.remove(checked);
      rethrow;
    } finally {
      checked._depth--;
    }
  }

  /// Waits until every connection opened so far has closed (up to
  /// [timeout]), then fails the test with the violations, if any.
  Future<void> verify({Duration timeout = const Duration(seconds: 5)}) async {
    final deadline = DateTime.now().add(timeout);
    while (_listeners.any((l) => !l._closed) &&
        DateTime.now().isBefore(deadline)) {
      await Future<void>.delayed(const Duration(milliseconds: 20));
    }
    final open = [
      for (final l in _listeners)
        if (!l._closed) 'no onClosed within $timeout: ${l._url}',
    ];
    _listeners.clear();
    final all = [...violations, ...open];
    violations.clear();
    if (all.isNotEmpty) fail('Transport contract violated:\n${all.join('\n')}');
  }
}

/// Installs a [CheckedTransport] around the transport made by [create] for
/// each test, and verifies it after the test (registered first, so it
/// runs after the test's own tear-downs closed their clients).
void useCheckedTransport(WebSocketTransport Function() create) {
  late CheckedTransport checked;
  setUp(() {
    WebSocketTransports.use(checked = CheckedTransport(create()));
    addTearDown(() async {
      WebSocketTransports.use(null);
      await checked.verify();
    });
  });
}

final class _CheckedListener implements TransportListener {
  _CheckedListener(this._transport, this._inner, this._url);

  final CheckedTransport _transport;
  final TransportListener _inner;
  final String _url;
  _CheckedConnection? _connection;

  /// Connection methods (and `connect`) currently on the stack.
  int _depth = 0;
  bool _opened = false;
  bool _closed = false;
  int _drainsRequested = 0;

  void _check(String event, {bool needsOpen = false}) {
    void violation(String what) =>
        _transport.violations.add('$what: $event ($_url)');
    if (_depth > 0) {
      violation('synchronous from ${_connection?._method ?? 'connect'}');
    }
    if (_closed) violation('after onClosed');
    if (needsOpen && !_opened) violation('before onOpen');
  }

  @override
  void onOpen(String? protocol) {
    _check('onOpen');
    if (_opened) _transport.violations.add('onOpen twice ($_url)');
    _opened = true;
    _inner.onOpen(protocol);
  }

  @override
  void onTextBytes(Uint8List bytes) {
    _check('onTextBytes', needsOpen: true);
    _inner.onTextBytes(bytes);
  }

  @override
  void onTextString(String text) {
    _check('onTextString', needsOpen: true);
    _inner.onTextString(text);
  }

  @override
  void onJson(Object? value) {
    _check('onJson', needsOpen: true);
    _inner.onJson(value);
  }

  @override
  void onBinary(Uint8List bytes) {
    _check('onBinary', needsOpen: true);
    _inner.onBinary(bytes);
  }

  @override
  void onPong() {
    _check('onPong', needsOpen: true);
    _inner.onPong();
  }

  @override
  void onDrain() {
    _check('onDrain', needsOpen: true);
    if (_drainsRequested == 0) {
      _transport.violations.add('onDrain without requestDrain ($_url)');
    }
    _drainsRequested = 0;
    _inner.onDrain();
  }

  @override
  void onClosed(int? code, String reason, WebSocketException? error) {
    _check('onClosed');
    _closed = true;
    _inner.onClosed(code, reason, error);
  }
}

final class _CheckedConnection implements TransportConnection {
  _CheckedConnection(this._inner, this._listener);

  final TransportConnection _inner;
  final _CheckedListener _listener;
  String? _method;

  R _call<R>(String method, R Function() body) {
    final outer = _method;
    _method = method;
    _listener._depth++;
    try {
      return body();
    } finally {
      _listener._depth--;
      _method = outer;
    }
  }

  @override
  void sendText(String text) => _call('sendText', () => _inner.sendText(text));

  @override
  void sendTextBytes(Uint8List bytes) =>
      _call('sendTextBytes', () => _inner.sendTextBytes(bytes));

  @override
  void sendBinary(Uint8List bytes) =>
      _call('sendBinary', () => _inner.sendBinary(bytes));

  @override
  void ping() => _call('ping', _inner.ping);

  @override
  void close(int? code, String? reason) =>
      _call('close', () => _inner.close(code, reason));

  @override
  void abort() => _call('abort', _inner.abort);

  @override
  int get bufferedAmount =>
      _call('bufferedAmount', () => _inner.bufferedAmount);

  @override
  void requestDrain() {
    _listener._drainsRequested++;
    _call('requestDrain', _inner.requestDrain);
  }
}

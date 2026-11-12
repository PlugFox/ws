// Plain data: states, exceptions, metrics and options (TEST_PLAN C1).
import 'package:test/test.dart';
import 'package:ws/ws.dart';

void main() {
  final url = Uri.parse('wss://h/p');
  const cause = WebSocketConnectionException('refused');

  group('states', () {
    test('fields, flags and toString', () {
      const disconnected = WebSocketState.disconnected(
        closeCode: 1001,
        closeReason: 'away',
        error: cause,
      );
      expect(disconnected, isA<WebSocketState$Disconnected>());
      expect((disconnected as WebSocketState$Disconnected).closeReason, 'away');
      expect(
        disconnected.toString(),
        'WebSocketState.disconnected(1001, '
        'WebSocketConnectionException: refused)',
      );
      expect(
        const WebSocketState.disconnected().toString(),
        'WebSocketState.disconnected()',
      );

      final connecting = WebSocketState.connecting(url: url, attempt: 3);
      expect(
        connecting.toString(),
        'WebSocketState.connecting($url, attempt 3)',
      );
      expect(connecting.isOpen || connecting.isClosed, isFalse);

      final open = WebSocketState.open(url: url, protocol: 'v1');
      expect(open.isOpen, isTrue);
      expect((open as WebSocketState$Open).protocol, 'v1');
      expect(open.toString(), 'WebSocketState.open($url)');

      const reconnecting = WebSocketState.reconnecting(
        attempt: 2,
        delay: Duration(milliseconds: 1500),
        cause: cause,
      );
      expect(
        reconnecting.toString(),
        'WebSocketState.reconnecting(attempt 2 in 1500 ms: '
        'WebSocketConnectionException: refused)',
      );

      const disconnecting = WebSocketState.disconnecting(
        code: 1000,
        reason: 'bye',
      );
      expect((disconnecting as WebSocketState$Disconnecting).reason, 'bye');
      expect(disconnecting.toString(), 'WebSocketState.disconnecting(1000)');
      expect(
        const WebSocketState.disconnecting().toString(),
        'WebSocketState.disconnecting()',
      );

      const closed = WebSocketState.closed();
      expect(closed.isClosed, isTrue);
      expect(closed.isOpen, isFalse);
      expect(closed.toString(), 'WebSocketState.closed');
    });

    test('switch over the sealed hierarchy is exhaustive', () {
      String name(WebSocketState s) => switch (s) {
        WebSocketState$Disconnected() => 'disconnected',
        WebSocketState$Connecting() => 'connecting',
        WebSocketState$Open() => 'open',
        WebSocketState$Reconnecting() => 'reconnecting',
        WebSocketState$Disconnecting() => 'disconnecting',
        WebSocketState$Closed() => 'closed',
      };
      expect(name(const WebSocketState.closed()), 'closed');
    });
  });

  group('exceptions', () {
    test('every type names itself and keeps its cause', () {
      final cases = <WebSocketException, String>{
        const WebSocketHandshakeException(
          'no upgrade',
          statusCode: 403,
          headers: {'x': 'y'},
        ): 'WebSocketHandshakeException: no upgrade',
        const WebSocketConnectionException('down', cause: 'socket'):
            'WebSocketConnectionException: down (cause: socket)',
        const WebSocketTimeoutException('slow'):
            'WebSocketTimeoutException: slow',
        const WebSocketClosedException('closed', code: 1011, reason: 'boom'):
            'WebSocketClosedException: closed',
        const WebSocketNotConnectedException('not yet'):
            'WebSocketNotConnectedException: not yet',
        const WebSocketFormatException('bad json'):
            'WebSocketFormatException: bad json',
        const WebSocketUnsupportedException('ping'):
            'WebSocketUnsupportedException: ping',
        const WebSocketBufferFullException('full', buffered: 42):
            'WebSocketBufferFullException: full',
        const WebSocketRpcException(
          'late',
          failure: WebSocketRpcFailure.timeout,
          id: 7,
        ): 'WebSocketRpcException: late',
      };
      for (final MapEntry(key: e, value: text) in cases.entries) {
        expect(e.toString(), text, reason: '${e.runtimeType}');
        expect(e, isA<Exception>());
      }
    });

    test('fields', () {
      const handshake = WebSocketHandshakeException(
        'x',
        statusCode: 429,
        headers: {'retry-after': '5'},
      );
      expect(handshake.statusCode, 429);
      expect(handshake.headers['retry-after'], '5');
      const closed = WebSocketClosedException(
        'x',
        code: 4000,
        reason: 'r',
        byServer: false,
      );
      expect((closed.code, closed.reason, closed.byServer), (4000, 'r', false));
      const rpc = WebSocketRpcException(
        'x',
        failure: WebSocketRpcFailure.remote,
        id: 'a',
        error: {'code': 1},
      );
      expect(rpc.failure, WebSocketRpcFailure.remote);
      expect(rpc.error, {'code': 1});
      expect(const WebSocketBufferFullException('x', buffered: 9).buffered, 9);
    });
  });

  test('metrics toString', () {
    final metrics = WebSocketMetrics(
      transport: 'fake',
      messagesSent: 1,
      messagesReceived: 2,
      bytesSent: 3,
      bytesReceived: 4,
      connects: 5,
      reconnectAttempts: 6,
      decodeErrors: 0,
      droppedReplies: 0,
      pendingRpc: 0,
      bufferedAmount: 0,
      lastRoundTrip: const Duration(milliseconds: 3),
      lastError: cause,
      openedAt: DateTime.utc(2026),
    );
    expect(
      metrics.toString(),
      'WebSocketMetrics(transport: fake, sent: 1, received: 2, '
      'connects: 5, reconnects: 6)',
    );
  });

  group('proxy', () {
    test('parse', () {
      final p = WebSocketProxy.parse('http://us%3Aer:p%40ss:word@proxy:3128');
      expect(
        (p.host, p.port, p.username, p.password),
        ('proxy', 3128, 'us:er', 'p@ss:word'),
      );
      // base64("us:er:p@ss:word")
      expect(p.authorization, 'Basic dXM6ZXI6cEBzczp3b3Jk');
      expect(p.toString(), 'WebSocketProxy(proxy:3128)');

      final bare = WebSocketProxy.parse('10.0.0.1:8080');
      expect(
        (bare.host, bare.port, bare.authorization),
        ('10.0.0.1', 8080, null),
      );
      expect(WebSocketProxy.parse('http://h').port, 80);
      expect(WebSocketProxy.parse('http://[::1]:1').host, '::1');
      expect(WebSocketProxy.parse('http://u@h').authorization, 'Basic dTo=');
    });

    test('rejects other schemes and empty hosts', () {
      expect(() => WebSocketProxy.parse('socks5://h:1'), throwsFormatException);
      expect(() => WebSocketProxy.parse('http://:1'), throwsFormatException);
    });
  });
}

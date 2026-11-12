import 'dart:async';
import 'dart:convert' show ascii, latin1;
import 'dart:io' as io;
import 'dart:typed_data';

import 'package:ws/src/client/exceptions.dart';
import 'package:ws/src/client/options.dart';
import 'package:ws/src/protocol/handshake.dart';

/// Proxy response heads larger than this are rejected.
const int _maxProxyHead = 16 * 1024;

/// The `CONNECT` request for [host]:[port] (RFC 9110 §9.3.6).
Uint8List connectRequest(String host, int port, WebSocketProxy proxy) {
  final authority = host.contains(':') ? '[$host]:$port' : '$host:$port';
  final authorization = proxy.authorization;
  return ascii.encode(
    'CONNECT $authority HTTP/1.1\r\n'
    'Host: $authority\r\n'
    '${authorization == null ? '' : 'Proxy-Authorization: $authorization\r\n'}'
    '\r\n',
  );
}

/// Asks the proxy behind [socket] to open a tunnel to [host]:[port].
///
/// Completes with the socket's subscription, idle, once the proxy
/// answered 2xx; the caller re-attaches its handlers (or hands the
/// subscription to `RawSecureSocket.secure`). Fails with a
/// [WebSocketConnectionException] otherwise.
Future<StreamSubscription<io.RawSocketEvent>> openTunnel(
  io.RawSocket socket,
  String host,
  int port,
  WebSocketProxy proxy,
) {
  final completer = Completer<StreamSubscription<io.RawSocketEvent>>();
  final request = connectRequest(host, port, proxy);
  var written = socket.write(request);
  if (written < request.length) socket.writeEventsEnabled = true;
  final head = BytesBuilder(copy: false);
  late final StreamSubscription<io.RawSocketEvent> subscription;

  void fail(String message, [Object? cause]) {
    if (completer.isCompleted) return;
    subscription.cancel().ignore();
    completer.completeError(
      WebSocketConnectionException(message, cause: cause),
    );
  }

  subscription = socket.listen(
    (event) {
      if (completer.isCompleted) return;
      switch (event) {
        case io.RawSocketEvent.write:
          written += socket.write(request, written);
          if (written < request.length) socket.writeEventsEnabled = true;
        case io.RawSocketEvent.read:
          final data = socket.read();
          if (data == null || data.isEmpty) return;
          head.add(data);
          final bytes = head.toBytes();
          final end = HandshakeResponse.headEnd(bytes);
          if (end < 0) {
            if (bytes.length > _maxProxyHead) {
              fail('Proxy response is too large');
            }
            return;
          }
          final statusLine = latin1
              .decode(Uint8List.sublistView(bytes, 0, end))
              .split('\r\n')
              .first;
          final status = int.tryParse(
            statusLine.split(' ').elementAtOrNull(1) ?? '',
          );
          if (status == null || status < 200 || status > 299) {
            return fail('Proxy refused CONNECT: $statusLine');
          }
          // The server speaks only after our request (WebSocket handshake
          // or TLS ClientHello), so nothing may follow the proxy's head.
          if (end + 4 != bytes.length) {
            return fail('Proxy sent data after its response');
          }
          // Not paused: RawSecureSocket.secure rejects a paused
          // subscription. Nothing is lost: the caller attaches its handlers
          // in the microtask that resumes it, before the next socket event.
          subscription.onData((_) {});
          completer.complete(subscription);
        case io.RawSocketEvent.readClosed:
        case io.RawSocketEvent.closed:
          fail('Proxy closed the connection');
      }
    },
    onError: (Object e) => fail('Proxy connection failed', e),
    onDone: () => fail('Proxy closed the connection'),
    cancelOnError: true,
  );
  return completer.future;
}

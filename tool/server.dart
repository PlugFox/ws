import 'dart:async';
import 'dart:io' as io;
import 'dart:isolate';
import 'dart:math' as math;

import 'package:shelf/shelf.dart' show Request, Response;
import 'package:shelf/shelf_io.dart' as shelf_io show serve;
import 'package:shelf_web_socket/shelf_web_socket.dart' show webSocketHandler;
import 'package:web_socket_channel/web_socket_channel.dart'
    show WebSocketChannel;

void main() => Future<void>(() async {
      _$shutdownHandler().whenComplete(() => io.exit(0)).ignore();
      print('Press Ctrl+C to exit.');
      final cpu = math.max(io.Platform.numberOfProcessors, 2);
      final address = io.InternetAddress.anyIPv4;
      const port = 8080;
      for (var i = 1; i <= cpu; i++) {
        final args = (address: address, port: port);
        Isolate.spawn<({io.InternetAddress address, int port})>(
          _$server,
          args,
          debugName: 'server-$i',
        );
      }
      print('Serving $cpu handlers at ws://localhost:$port');
    });

void _$server(({io.InternetAddress address, int port}) args) => shelf_io.serve(
      _$websocketHandler(),
      args.address,
      args.port,
      poweredByHeader: 'WS Server #${Isolate.current.debugName ?? 'unknown'}',
      shared: true,
    );

FutureOr<Response> Function(Request) _$websocketHandler() =>
    webSocketHandler((WebSocketChannel webSocket) {
      void push(Object message) {
        print('< $message');
        webSocket.sink.add(message);
      }

      webSocket.stream.listen((Object? message) {
        print('> $message');
        switch (message) {
          case "ping":
            push("pong");
            break;
        }
      });
    });

Future<void> _$shutdownHandler<T extends Object?>() {
  //StreamSubscription<String>? userKeySub;
  StreamSubscription<io.ProcessSignal>? sigIntSub;
  StreamSubscription<io.ProcessSignal>? sigTermSub;
  final shutdownCompleter = Completer<T>.sync();
  var catchShutdownEvent = false;
  {
    Future<void> signalHandler(io.ProcessSignal signal) async {
      if (catchShutdownEvent) return;
      catchShutdownEvent = true;
      print('Received signal [$signal] - closing');
      T? result;
      try {
        //userKeySub?.cancel();
        sigIntSub?.cancel().ignore();
        sigTermSub?.cancel().ignore();
      } finally {
        shutdownCompleter.complete(result);
      }
    }

    sigIntSub = io.ProcessSignal.sigint
        .watch()
        .listen(signalHandler, cancelOnError: false);
    // SIGTERM is not supported on Windows.
    // Attempting to register a SIGTERM handler raises an exception.
    if (!io.Platform.isWindows) {
      sigTermSub = io.ProcessSignal.sigterm
          .watch()
          .listen(signalHandler, cancelOnError: false);
    }
  }
  return shutdownCompleter.future;
}

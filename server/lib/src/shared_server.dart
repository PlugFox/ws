import 'dart:async';
import 'dart:io' as io;
import 'dart:isolate';

import 'package:shelf/shelf.dart';
import 'package:shelf/shelf_io.dart' as shelf_io;
import 'package:ws_server/src/middleware/cors.dart';
import 'package:ws_server/src/middleware/errors.dart';
import 'package:ws_server/src/middleware/injector.dart';
import 'package:ws_server/src/router/rest_router.dart';
import 'package:ws_server/src/router/websocket.dart';
import 'package:ws_server/src/util/logger.dart';

typedef SharedServerSetup = ({
  Isolate isolate,
  void Function(Object? event) send,
  Stream<Object?> stream
});

typedef _SharedServerArguments = ({
  io.InternetAddress address,
  int port,
  SendPort sendPort,
  String label
});

class SharedServer {
  SharedServer({
    required this.connection,
    required this.label,
  });

  final ({io.InternetAddress address, int port}) connection;
  final String label;

  Future<SharedServerSetup> call() async {
    final receivePort = ReceivePort();
    final controller = StreamController<Object?>();
    try {
      final responsePort = receivePort.sendPort;
      final sendPortCompleter = Completer<SendPort>();

      // Listen to isolate
      var isAlive = true;
      receivePort.listen(
        (Object? message) {
          switch (message) {
            case SendPort sendPort:
              sendPortCompleter.complete(sendPort);
              break;
            case 0:
              // Isolate is alive
              isAlive = true;
              break;
            default:
              controller.add(message);
              break;
          }
        },
        cancelOnError: false,
      );

      // Spawn isolate
      final isolate = await Isolate.spawn<_SharedServerArguments>(
        _endpoint,
        (
          address: connection.address,
          port: connection.port,
          sendPort: responsePort,
          label: label
        ),
        debugName: 'Isolate($label)',
        errorsAreFatal: true,
      );

      // Wait for sendPort
      final sendPort =
          await sendPortCompleter.future.timeout(const Duration(seconds: 5));

      // Health check
      late final Timer timer;

      // Exit process if isolate is stuck
      Future<void> onStuck() async {
        severe('Isolate($label) is stuck');
        timer.cancel();
        receivePort.close();
        controller.close().ignore();
        isolate.kill(priority: Isolate.immediate);
        await Future<void>.delayed(const Duration(seconds: 5));
        io.exit(1);
      }

      timer = Timer.periodic(const Duration(seconds: 15), (_) {
        try {
          if (!isAlive) {
            onStuck().ignore();
            return;
          }
          isolate.ping(responsePort, response: 0);
          isAlive = false;
        } on Object {
          onStuck();
        }
      });
      return (isolate: isolate, send: sendPort.send, stream: controller.stream);
    } on Object {
      receivePort.close();
      controller.close().ignore();
      rethrow;
    }
  }

  /// The entry point for the isolate.
  static void _endpoint(_SharedServerArguments args) => Future<void>(() async {
        //fine('Starting isolate ${Isolate.current.debugName ?? 'unknown'}');
        final receivePort = ReceivePort();
        args.sendPort.send(receivePort.sendPort);
        receivePort.listen((Object? message) {
          print('Isolate(${args.label}) received: $message'); /* ... */
        });
        final http = Pipeline()
            .addMiddleware(handleErrors())
            .addMiddleware(
              logRequests(
                logger: (msg, isError) => isError ? warning(msg) : fine(msg),
              ),
            )
            .addMiddleware(cors())
            .addMiddleware(injector(<String, Object>{}))
            .addHandler($restRouter);
        final ws = $webSocket;
        // ignore: unused_local_variable
        final server = await shelf_io.serve(
          // If path equals 'ws' then use websockets, otherwise use http
          // e.g. ws://localhost:8080/connect
          (request) =>
              request.url.path == 'connect' ? ws(request) : http(request),
          args.address,
          args.port,
          poweredByHeader: 'WS Server #${args.label}',
          shared: true,
        );
        //config('Server running on ${server.address}:${server.port}');
      });
}

// ignore_for_file: unused_field

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

  Isolate? _isolate;
  void Function(Object? event)? _send;
  Stream<Object?>? _stream;

  Future<void> call() async {
    final receivePort = ReceivePort();
    final controller = StreamController<Object?>();
    if (_isolate != null) throw StateError('Isolate is already running');
    try {
      final responsePort = receivePort.sendPort;
      final sendPortCompleter = Completer<SendPort>();

      var isAlive = true;
      late Isolate isolate;
      late final Timer timer;

      // Exit process if isolate is stuck
      Future<void> close() async {
        _isolate = null;
        _send = null;
        _stream = null;
        timer.cancel();
        receivePort.close();
        controller.close().ignore();
        isolate.kill(priority: Isolate.immediate);
        await Future<void>.delayed(const Duration(seconds: 1));
        io.exit(1);
      }

      // Listen to isolate
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
            case 1:
              // Isolate is dead
              isAlive = false;
              close();
              break;
            default:
              controller.add(message);
              break;
          }
        },
        cancelOnError: false,
      );

      // Spawn isolate
      isolate = await Isolate.spawn<_SharedServerArguments>(
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

      timer = Timer.periodic(const Duration(seconds: 15), (_) {
        try {
          if (!isAlive) {
            severe('Isolate($label) is stuck');
            close().ignore();
            return;
          }
          isolate.ping(responsePort, response: 0);
          isAlive = false;
        } on Object {
          severe('Unknown health check error');
          close();
        }
      });

      _isolate = isolate;
      _send = sendPort.send;
      _stream = controller.stream;
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
        io.HttpServer? server;

        // Close server and isolate
        void close() {
          receivePort.close();
          server?.close(force: true).whenComplete(() {
            args.sendPort.send(1);
            Isolate.current.kill(priority: Isolate.beforeNextEvent);
          });
        }

        receivePort.listen((Object? message) {
          /* ... */
          switch (message) {
            case 0:
              // Isolate is alive
              break;
            case 2:
              // We should close the server
              close();
              break;
            default:
              break;
          }
        });
        final handler = Pipeline()
            .addMiddleware(handleErrors())
            .addMiddleware(injector(<String, Object>{}))
            .addMiddleware(webSocket(path: '/connect'))
            .addMiddleware(
              logRequests(
                logger: (msg, isError) => isError ? warning(msg) : fine(msg),
              ),
            )
            .addMiddleware(corsHeaders())
            .addHandler($restRouter);
        try {
          server = await shelf_io.serve(
            handler,
            args.address,
            args.port,
            poweredByHeader: 'WS Server #${args.label}',
            shared: true,
          );
        } on Object {
          close();
        }
      });
}

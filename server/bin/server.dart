// @dart=3.0

import 'dart:async';
import 'dart:io' as io;

import 'package:shelf/shelf.dart' as shelf;
import 'package:shelf/shelf_io.dart' as shelf_io;
import 'package:ws_server/src/middleware/handle_errors.dart';
import 'package:ws_server/src/middleware/injector.dart';
import 'package:ws_server/src/middleware/log_pipeline.dart';
import 'package:ws_server/src/router/rest_router.dart';
import 'package:ws_server/src/router/websocket.dart';
import 'package:ws_server/src/util/cors.dart';
import 'package:ws_server/src/util/run_server.dart';

/// Starts the server.
/// dart run server/bin/server.dart
void main(List<String> arguments) => runServer<void>(
      config: null,
      serve: _serve,
      arguments: arguments,
    );

/// The entry point for the isolate.
void _serve(io.InternetAddress address, int port, [config]) =>
    Future<void>(() async {
      //fine('Starting isolate ${Isolate.current.debugName ?? 'unknown'}');
      final pipeline = shelf.Pipeline()
          .addMiddleware(handleErrors())
          .addMiddleware(injector(<String, Object>{}))
          .addMiddleware(webSocket(path: '/connect'))
          .addMiddleware(logPipeline())
          .addMiddleware(cors())
          .addHandler($restRouter);
      await shelf_io.serve(
        pipeline,
        address,
        port,
        poweredByHeader: 'WS Server',
        shared: true,
      );
    });

// @dart=3.0

import 'dart:async';
import 'dart:io' as io;

import 'package:ws_server/src/shared_server.dart';
import 'package:ws_server/src/util/options.dart';
import 'package:ws_server/src/util/shutdown_handler.dart';

/// Starts the server.
/// dart run server/bin/server.dart
void main(List<String> arguments) => Future<void>(() async {
      // Allow shutdown via Ctrl+C
      $shutdownHandler().whenComplete(() => io.exit(0)).ignore();
      print('Press Ctrl+C to exit.');

      // Extract startup options
      final options = $extractOptions(arguments);

      // Isolate pool
      final connection = (
        address: io.InternetAddress.anyIPv4,
        port: options.port,
      );
      for (var i = 1; i <= options.isolates; i++) {
        SharedServer(connection: connection, label: 'Server-$i')();
      }
      print(
        'Serving ${options.isolates} handlers at '
        'http://${connection.address.host}:${connection.port}',
      );
    });

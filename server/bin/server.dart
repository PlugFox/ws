// @dart=3.0

import 'dart:async';
import 'dart:io' as io;
import 'dart:math' as math;

import 'package:ws_server/src/shared_server.dart';
import 'package:ws_server/src/util/shutdown_handler.dart';

/// Starts the server.
/// dart run server/bin/server.dart
void main([List<String>? arguments]) => Future<void>(() async {
      $shutdownHandler().whenComplete(() => io.exit(0)).ignore();
      print('Press Ctrl+C to exit.');
      final cpu = math.max(io.Platform.numberOfProcessors, 2);
      final connection = (address: io.InternetAddress.anyIPv4, port: 8080);
      for (var i = 1; i <= cpu; i++) {
        // ignore: unused_local_variable
        final setup = SharedServer(
          connection: connection,
          label: 'Server-$i',
        )();
      }
      print(
        'Serving $cpu handlers at '
        'ws://${connection.address.host}:${connection.port}',
      );
    });

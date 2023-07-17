import 'dart:async';
import 'dart:io' as io;
import 'dart:isolate';

import 'package:args/args.dart';
import 'package:l/l.dart';
import 'package:ws_server/src/util/log_printer.dart';

/// Returns a [Future] whith a [StreamController] that can be used to send and
/// receive messages to/from all isolates.
Future<void> runServer<Config>({
  /// Config to pass to the each isolate.
  required Config config,

  /// Start the server on the given address.
  required FutureOr<void> Function(
    /// Start the server on the given address.
    io.InternetAddress address,

    /// Start the server on the given port.
    int port,

    /// Config to pass to the each isolate.
    Config config,
  ) serve,

  /// Arguments to parse.
  List<String>? arguments,
}) async {
  final stopwatch = Stopwatch()..start();
  final parsedArguments = _parseArguments(arguments);
  if (parsedArguments.help == true) {
    // ignore: avoid_print
    print(_help);
    io.exit(0);
  }

  l.capture<void>(
    () => runZonedGuarded<void>(
      () async {
        _shutdownHandler(() => io.exit(0)).ignore();
        final ports = <SendPort>[];
        for (var i = 1; i <= parsedArguments.isolates; i++) {
          final receivePort = ReceivePort();
          SendPort? sendPort;
          final server = SharedWorker<Config>(
            address: parsedArguments.address,
            port: parsedArguments.port,
            sendPort: receivePort.sendPort,
            label: 'Worker#$i',
            config: config,
            serve: serve,
          );
          final isolate = await server();
          final completer = Completer<void>.sync();
          receivePort.listen(
            (message) {
              switch (message) {
                case SendPort port:
                  ports.add(sendPort = port);
                  if (!completer.isCompleted) completer.complete();
                case LogMessage msg:
                  msg();
                case [String error, String? stackTrace]:
                  ports.remove(sendPort);
                  isolate.kill();
                  l.e(
                    'FATAL ERROR AT ${server.label} | $error',
                    switch (stackTrace) {
                      String string when string.isNotEmpty =>
                        StackTrace.fromString(string),
                      _ => null,
                    },
                  );
                  io.exit(2);
                default:
                  /* onMessage?.call(message); */
                  break;
              }
            },
            cancelOnError: false,
          );
          await completer.future;
        }
        /* messages.listen(
          (message) => ports.forEach((port) => port.send(message)),
          cancelOnError: false,
        ); */
        // TODO(plugfox): Health checks
        l.s(
          'Started ${parsedArguments.isolates} server(s) '
          'at http://${parsedArguments.address.address}:${parsedArguments.port} '
          'in ${(stopwatch..stop()).elapsedMilliseconds} ms.',
        );
      },
      (error, stackTrace) async {
        l.e('FATAL ERROR | $error', stackTrace);
        io.exit(2);
      },
    ),
    LogOptions(
      handlePrint: true,
      outputInRelease: true,
      printColors: false,
      overrideOutput:
          $logPrinter(parsedArguments.environment, parsedArguments.verbose),
    ),
  );
}

({
  bool help,
  int verbose,
  io.InternetAddress address,
  int port,
  String environment,
  int isolates,
}) _parseArguments(List<String>? arguments) {
  final argResult = (ArgParser()
        ..addFlag('help',
            abbr: 'h', negatable: false, help: 'Print this usage information.')
        ..addOption(
          'verbose',
          abbr: 'v',
          valueHelp: '3',
          help: 'Verbosity level.',
          defaultsTo: io.Platform.environment['VERBOSE_LEVEL'] ??
              const String.fromEnvironment('VERBOSE_LEVEL', defaultValue: '3'),
        )
        ..addOption(
          'address',
          abbr: 'a',
          valueHelp: '127.0.0.1:8080',
          help: 'Address to listen on.',
          defaultsTo: io.Platform.environment['ADDRESS'] ??
              const String.fromEnvironment('ADDRESS',
                  defaultValue: '127.0.0.1'),
        )
        ..addOption(
          'port',
          abbr: 'p',
          valueHelp: '8080',
          help: 'Port to listen on.',
          defaultsTo: io.Platform.environment['PORT'] ??
              const String.fromEnvironment('PORT', defaultValue: '8080'),
        )
        ..addOption(
          'isolates',
          abbr: 'i',
          valueHelp: '6',
          help: 'Number of isolates to run.',
          defaultsTo: io.Platform.environment['ISOLATES'] ??
              const String.fromEnvironment('ISOLATES', defaultValue: '0'),
        )
        ..addOption('environment',
            abbr: 'e',
            valueHelp: 'staging',
            help: 'Environment to run in.',
            defaultsTo: io.Platform.environment['ENVIRONMENT'] ??
                const String.fromEnvironment('ENVIRONMENT',
                    defaultValue: 'production')))
      .parse(arguments ?? const <String>[]);
  return (
    help: argResult['help'] ?? false,
    verbose: int.tryParse(argResult['verbose']?.toString() ?? '3') ?? 3,
    address: switch (argResult['address']) {
      'any' => io.InternetAddress.anyIPv4,
      '127.0.0.1' => io.InternetAddress.loopbackIPv4,
      '0.0.0.0' => io.InternetAddress.anyIPv4,
      'loopback' => io.InternetAddress.loopbackIPv4,
      'localhost' => io.InternetAddress.loopbackIPv4,
      'loopbackIPv4' => io.InternetAddress.loopbackIPv4,
      'loopbackIPv6' => io.InternetAddress.loopbackIPv6,
      'anyIPv4' => io.InternetAddress.anyIPv4,
      'anyIPv6' => io.InternetAddress.anyIPv6,
      String address => io.InternetAddress(address),
      _ => io.InternetAddress.anyIPv4,
    },
    port: (int.tryParse(argResult['port']?.toString() ?? '8080') ?? 8080)
        .clamp(0, 65535),
    environment: argResult['environment'] ?? 'production',
    isolates: switch (int.tryParse(argResult['isolates']?.toString() ?? '0')) {
      int _ && < 1 => io.Platform.numberOfProcessors,
      int i && > 0 => i,
      null || _ => io.Platform.numberOfProcessors,
    },
  );
}

const String _help = '''
-h, --help                  Prints usage information.

-v, --verbose=<level>       Sets the level of logging verbosity.
                            By default, if not specified otherwise in the environment variables, it is set to '3'.

-a, --address=<address>     Sets the address for the server to listen on.
                            By default, if not specified otherwise in the environment variables, it is set to '127.0.0.1'.

-p, --port=<port>           Sets the port for the server to listen on.
                            By default, if not specified otherwise in the environment variables, it is set to '8080'.

-i, --isolates=<number>     Sets the number of isolates to run.
                            Isolates in Dart are units of parallelism that allow for multitasking.
                            By default, if not specified otherwise in the environment variables, the value is '-1'.

-e, --environment=<env>     Defines the runtime environment.
                            By default, if not specified otherwise in the environment variables, it is set to 'production'.
''';

/// Приготовимся к завершению приложения
Future<T?> _shutdownHandler<T extends Object?>([
  final Future<T> Function()? onShutdown,
]) {
  l.i('Press [Ctrl+C] to exit');
  //StreamSubscription<String>? userKeySub;
  StreamSubscription<io.ProcessSignal>? sigIntSub;
  StreamSubscription<io.ProcessSignal>? sigTermSub;
  final shutdownCompleter = Completer<T>.sync();
  var catchShutdownEvent = false;
  {
    Future<void> signalHandler(io.ProcessSignal signal) async {
      if (catchShutdownEvent) return;
      catchShutdownEvent = true;
      l.i('Received signal "$signal" - closing');
      T? result;
      try {
        //userKeySub?.cancel();
        sigIntSub?.cancel().ignore();
        sigTermSub?.cancel().ignore();
        result = await onShutdown
            ?.call()
            .catchError((Object error, StackTrace stackTrace) {
          l.e('Error during shutdown | $error', stackTrace);
          io.exit(2);
        });
      } finally {
        if (!shutdownCompleter.isCompleted) shutdownCompleter.complete(result);
      }
    }

    // Ошибка в проде при попытке отслеживания событий с клавиатуры
    // StdinException: Error setting terminal echo mode,
    // OS Error: Inappropriate ioctl for device, errno = 25
    /*
    if (io.stdin.hasTerminal) {
      l.i('Press [Q] to exit');
      io.stdin.echoMode = false;
      io.stdin.lineMode = false;
      userKeySub = const Utf8Decoder().bind(io.stdin).listen(
        (line) {
          final formattedLine = line.trim().toLowerCase();
          if (formattedLine.contains('q')) {
            signalHandler(io.ProcessSignal.sigint);
          } else {
            l.i('Press [Q] to exit');
          }
        },
      );
    }
    */

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

class SharedWorker<Config> {
  SharedWorker({
    required this.address,
    required this.port,
    required this.sendPort,
    required this.label,
    required this.config,
    required this.serve,
  });

  final io.InternetAddress address;
  final int port;
  final SendPort sendPort;
  final String label;

  final Config config;
  final FutureOr<void> Function(
    io.InternetAddress address,
    int port,
    Config config,
  ) serve;

  Future<Isolate> call() => Isolate.spawn<SharedWorker<Config>>(
        _endpoint,
        this,
        debugName: label,
        errorsAreFatal: true,
      );

  /// The entry point for the isolate.
  static void _endpoint<Config>(SharedWorker<Config> args) {
    final logPrinter = $sendToPort(args.sendPort, 3);
    l.capture<void>(
      () => runZonedGuarded<void>(
        () async {
          final receivePort = ReceivePort()..listen((message) {/* ... */});
          await args.serve(args.address, args.port, args.config);
          args.sendPort.send(receivePort.sendPort);
        },
        (error, stackTrace) => args.sendPort
            .send(<String>[error.toString(), stackTrace.toString()]),
      ),
      LogOptions(
        handlePrint: true,
        outputInRelease: true,
        printColors: false,
        overrideOutput: logPrinter,
      ),
    );
  }
}

import 'dart:async';
import 'dart:io' as io;

Future<void> $shutdownHandler<T extends Object?>() {
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

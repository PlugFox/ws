import 'package:l/l.dart';
import 'package:shelf/shelf.dart' as shelf;

shelf.Middleware logPipeline() => shelf.logRequests(
      logger: (msg, isError) => isError ? l.w(msg) : l.i(msg),
    );

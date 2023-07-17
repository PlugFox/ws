import 'package:shelf/shelf.dart';
import 'package:shelf_router/shelf_router.dart';
import 'package:ws_server/src/router/health_check.dart';
import 'package:ws_server/src/router/long_polling.dart';
import 'package:ws_server/src/router/not_found.dart';

final Handler $restRouter = Router(notFoundHandler: $notFound)
  ..get('/health', $healthCheck)
  ..get('/long-polling', $longPolling);

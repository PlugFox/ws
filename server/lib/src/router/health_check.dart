import 'dart:async';

import 'package:shelf/shelf.dart' show Request, Response;
import 'package:ws_server/src/util/responses.dart';

FutureOr<Response> $healthCheck(Request request) => Responses.ok(null);

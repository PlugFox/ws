import 'dart:async';

import 'package:shelf/shelf.dart' as shelf;
import 'package:ws_server/src/util/responses.dart';

FutureOr<shelf.Response> $notFound(shelf.Request request) => Responses.error(
      NotFoundException(data: <String, Object?>{
        'path': request.url.path,
        'query': request.url.queryParameters,
        'method': request.method,
        'headers': request.headers,
      }),
    );

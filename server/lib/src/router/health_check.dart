import 'dart:io' as io;

import 'package:shelf/shelf.dart' show Request, Response;

Response $healthCheck(Request request) => Response.ok(
      '{"data": {"status": "ok"}}',
      headers: <String, String>{
        'Content-Type': io.ContentType.json.value,
      },
    );

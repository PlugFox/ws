import 'dart:convert';
import 'dart:io' as io;

import 'package:shelf/shelf.dart' show Request, Response;

Response $notFound(Request request) => Response.notFound(
      jsonEncode(<String, Object?>{
        'error': <String, Object?>{
          'status': io.HttpStatus.notFound,
          'message': 'Not Found',
          'details': <String, Object?>{
            'path': request.url.path,
            'method': request.method,
            'headers': request.headers,
          },
        },
      }),
      headers: <String, String>{
        'Content-Type': io.ContentType.json.value,
      },
    );

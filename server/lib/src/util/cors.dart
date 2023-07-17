import 'package:shelf/shelf.dart';

Middleware cors([Map<String, String>? headers]) =>
    (innerHandler) => (request) => Future<Response>.sync(() => innerHandler(request)).then(
          (response) => response.change(
            headers: <String, String>{
              ...response.headers,
              ...?headers,
              'Access-Control-Allow-Origin': '*',
              'Access-Control-Allow-Methods': 'GET, POST, HEAD, OPTIONS',
              'Access-Control-Allow-Headers': '*',
              'Access-Control-Allow-Credentials': 'true',
              'Access-Control-Max-Age': '86400',
              'Access-Control-Expose-Headers': '*',
              'Access-Control-Request-Headers': '*',
              'Access-Control-Request-Method': '*',
            },
          ),
        );

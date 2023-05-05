import 'package:shelf/shelf.dart';

/// Injects a [Map] of dependencies into the request context.
Middleware injector(Map<String, Object> dependency) =>
    (innerHandler) => (request) => innerHandler(
          request.change(
            context: <String, Object>{
              ...request.context,
              ...dependency,
            },
          ),
        );

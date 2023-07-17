import 'dart:convert';
import 'dart:io' as io;

import 'package:shelf/shelf.dart';
import 'package:stack_trace/stack_trace.dart' as st;
import 'package:ws_server/src/util/responses.dart';

/// Response encoder
final Converter<Map<String, Object?>, String> _responseEncoder =
    const JsonEncoder().cast<Map<String, Object?>, String>();

/// Middleware that catches all errors and sends a JSON response with the error
/// message. If the error is not an instance of [HttpException], it will be
/// wrapped into one with the status code 500.
Middleware handleErrors({bool showStackTrace = false}) => (Handler handler) =>
    (Request request) => Future.sync(() => handler(request))
            .then<Response>((Response response) => response)
            .catchError(
          (Object error, StackTrace? stackTrace) {
            final result = error is HttpException
                ? error
                : HttpException(
                    statusCode: io.HttpStatus.internalServerError,
                    code: 'internal',
                    message: 'Internal Server Error',
                    data: <String, Object?>{
                      'path': request.url.path,
                      'query': request.url.queryParameters,
                      'method': request.method,
                      'headers': request.headers,
                      'error': showStackTrace
                          ? error.toString()
                          : _errorRepresentation(error),
                      if (showStackTrace && stackTrace != null)
                        'stack_trace': st.Trace.format(stackTrace),
                    },
                  );
            return Response(
              result.statusCode,
              body: _responseEncoder.convert(result.toJson()),
              headers: <String, String>{
                'Content-Type': io.ContentType.json.value,
              },
            );
          },
        );

String _errorRepresentation(Object? error) => switch (error) {
      FormatException _ => 'Format exception',
      HttpException _ => 'HTTP exception',
      UnimplementedError _ => 'Unimplemented error',
      UnsupportedError _ => 'Unsupported error',
      RangeError _ => 'Range error',
      StateError _ => 'State error',
      ArgumentError _ => 'Argument error',
      TypeError _ => 'Type error',
      OutOfMemoryError _ => 'Out of memory error',
      StackOverflowError _ => 'Stack overflow error',
      _ => 'Unknown error',
    };

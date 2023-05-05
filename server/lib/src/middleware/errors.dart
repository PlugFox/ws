import 'dart:convert';
import 'dart:io' as io;

import 'package:shelf/shelf.dart';
import 'package:stack_trace/stack_trace.dart' as st;

/// Middleware that catches all errors and sends a JSON response with the error
/// message. If the error is not an instance of [HttpException], it will be
/// wrapped into one with the status code 500.
Middleware handleErrors() => (Handler handler) => (Request request) =>
    Future.sync(() => handler(request))
        .then<Response>((Response response) => response)
        .onError(
      (Object error, StackTrace stackTrace) {
        final result = error is HttpException
            ? error
            : HttpException(
                data: <String, Object?>{
                  'path': request.url.path,
                  'method': request.method,
                  'headers': request.headers,
                  'error': error.toString(),
                  'stack_trace': st.Trace.format(stackTrace),
                },
              );
        return Response(
          result.status,
          body: jsonEncode(result.toJson()),
          headers: <String, String>{
            'Content-Type': io.ContentType.json.value,
          },
        );
      },
      test: (Object error) => error is HijackException ? false : true,
    );

/// HTTP exception enables to immediately stop request execution
/// and send an appropriate error message to the client. An option
/// [Map] data can be provided to add additional information as
/// the response body.
class HttpException implements Exception {
  const HttpException({
    this.status = io.HttpStatus.internalServerError,
    this.message = "Internal Server Error",
    this.data,
  });

  final int status;
  final String message;
  final Map<String, Object?>? data;

  HttpException copyWith({
    int? status,
    String? message,
    Map<String, Object?>? data,
  }) =>
      HttpException(
        status: status ?? this.status,
        message: message ?? this.message,
        data: data ?? this.data,
      );

  Map<String, Object?> toJson() => <String, Object?>{
        'error': <String, Object?>{
          'status': status,
          'message': message,
          if (data != null) 'details': data,
        },
      };

  @override
  String toString() => "Status ${status.toString()}: $message";
}

// 400 Bad Request
class BadRequestException extends HttpException {
  const BadRequestException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.badRequest,
            message: "Bad Request${(detail != '' ? ': ' : '')}$detail");
}

// 401 Unauthorized
class UnauthorizedException extends HttpException {
  const UnauthorizedException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.unauthorized,
            message: "Unauthorized${(detail != '' ? ': ' : '')}$detail");
}

// 402 Payment Required
class PaymentRequiredException extends HttpException {
  const PaymentRequiredException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.paymentRequired,
            message: "Payment Required${(detail != '' ? ': ' : '')}$detail");
}

// 403 Forbidden
class ForbiddenException extends HttpException {
  const ForbiddenException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.forbidden,
            message: "Forbidden${(detail != '' ? ': ' : '')}$detail");
}

// 404 Not Found
class NotFoundException extends HttpException {
  const NotFoundException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.notFound,
            message: "Not Found${(detail != '' ? ': ' : '')}$detail");
}

// 405 Method Not Allowed
class MethodNotAllowed extends HttpException {
  const MethodNotAllowed({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.methodNotAllowed,
            message: "Method Not Allowed${(detail != '' ? ': ' : '')}$detail");
}

// 406 Not Acceptable
class NotAcceptableException extends HttpException {
  const NotAcceptableException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.notAcceptable,
            message: "Not Acceptable${(detail != '' ? ': ' : '')}$detail");
}

// 409 Conflict
class ConflictException extends HttpException {
  const ConflictException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.conflict,
            message: "Conflict${(detail != '' ? ': ' : '')}$detail");
}

// 410 Gone
class GoneException extends HttpException {
  const GoneException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.gone,
            message: "Gone${(detail != '' ? ': ' : '')}$detail");
}

// 412 Precondition Failed
class PreconditionFailedException extends HttpException {
  const PreconditionFailedException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.preconditionFailed,
            message: "Precondition Failed${(detail != '' ? ': ' : '')}$detail");
}

// 415 Unsupported Media Type
class UnsupportedMediaTypeException extends HttpException {
  const UnsupportedMediaTypeException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.unsupportedMediaType,
            message:
                "Unsupported Media Type${(detail != '' ? ': ' : '')}$detail");
}

// 429 Too Many Requests
class TooManyRequestsException extends HttpException {
  const TooManyRequestsException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.tooManyRequests,
            message: "Too Many Requests${(detail != '' ? ': ' : '')}$detail");
}

// 501 Not Implemented
class NotimplementedException extends HttpException {
  const NotimplementedException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.notImplemented,
            message: "Not Implemented${(detail != '' ? ': ' : '')}$detail");
}

// 503 Service Unavailable
class ServiceUnavailableException extends HttpException {
  const ServiceUnavailableException({super.data, String detail = ''})
      : super(
            status: io.HttpStatus.serviceUnavailable,
            message: "Service Unavailable${(detail != '' ? ': ' : '')}$detail");
}

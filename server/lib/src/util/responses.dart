// ignore_for_file: avoid_classes_with_only_static_members

import 'dart:async';
import 'dart:convert';
import 'dart:io' as io;

import 'package:shelf/shelf.dart' as shelf;

/// Statuses
const ({String ok, String error}) _kStatus = (ok: 'ok', error: 'error');

/// Headers
final Map<String, String> _headers = <String, String>{
  'Content-Type': io.ContentType.json.value,
  /* 'Cache-Control': 'public, max-age=86400', */
};

/// Responses
sealed class Responses {
  /// Response encoder
  static final Converter<Map<String, Object?>, List<int>> _responseEncoder =
      const JsonEncoder()
          .cast<Map<String, Object?>, String>()
          .fuse(const Utf8Encoder());

  /// Ok
  static FutureOr<shelf.Response> ok(
    Object? data, {
    Map<String, String>? headers,
  }) {
    List<int> body;
    switch (data) {
      case Map<String, Object?> data:
        body = _responseEncoder.convert(
          <String, Object>{
            'status': _kStatus.ok,
            'data': data,
          },
        );
      case null:
        body = utf8.encode('{"status":"${_kStatus.ok}"}');
      case List<int> data:
        body = data;
      case DateTime data:
        return ok({'value': data.toUtc().toIso8601String()}, headers: headers);
      case num data:
        return ok({'value': data}, headers: headers);
      case String data:
        return ok({'value': data}, headers: headers);
      case List<Object?> data:
        return ok({'value': data}, headers: headers);
      default:
        return error(
          const HttpException(
            statusCode: io.HttpStatus.internalServerError,
            code: 'internal',
            message: 'Internal Server Error',
            data: <String, Object?>{
              'error': 'Invalid response data type',
            },
          ),
        );
    }
    return shelf.Response.ok(
      body,
      headers: <String, String>{
        ..._headers,
        ...?headers,
        'Content-Length': body.length.toString(),
      },
    );
  }

  /// Error
  static FutureOr<shelf.Response> error(
    HttpException exception, {
    Map<String, String>? headers,
  }) {
    final body = _responseEncoder.convert(exception.toJson());
    return shelf.Response(
      exception.statusCode,
      body: body,
      headers: <String, String>{
        ..._headers,
        ...?headers,
        'Content-Length': body.length.toString(),
      },
    );
  }
}

/// HTTP exception enables to immediately stop request execution
/// and send an appropriate error message to the client. An option
/// [Map] data can be provided to add additional information as
/// the response body.
class HttpException implements Exception {
  const HttpException({
    this.statusCode = io.HttpStatus.internalServerError,
    this.code = 'internal',
    this.message = 'Internal Server Error',
    this.data,
  });

  final int statusCode;
  final String code;
  final String message;
  final Map<String, Object?>? data;

  HttpException copyWith({
    int? statusCode,
    String? message,
    Map<String, Object?>? data,
  }) =>
      HttpException(
        statusCode: statusCode ?? this.statusCode,
        message: message ?? this.message,
        data: data ?? this.data,
      );

  Map<String, Object?> toJson() => <String, Object?>{
        'status': _kStatus.error,
        'error': <String, Object?>{
          'statusCode': statusCode,
          'code': code,
          'message': message,
          if (data != null) 'details': data,
        },
      };

  @override
  String toString() => 'HttpException ${statusCode.toString()}: $message';
}

// 400 Bad Request
class BadRequestException extends HttpException {
  const BadRequestException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.badRequest,
            code: 'bad_request',
            message: "Bad Request${detail != '' ? ': ' : ''}$detail");
}

// 401 Unauthorized
class UnauthorizedException extends HttpException {
  const UnauthorizedException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.unauthorized,
            code: 'unauthorized',
            message: "Unauthorized${detail != '' ? ': ' : ''}$detail");
}

// 402 Payment Required
class PaymentRequiredException extends HttpException {
  const PaymentRequiredException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.paymentRequired,
            code: 'payment_required',
            message: "Payment Required${detail != '' ? ': ' : ''}$detail");
}

// 403 Forbidden
class ForbiddenException extends HttpException {
  const ForbiddenException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.forbidden,
            code: 'forbidden',
            message: "Forbidden${detail != '' ? ': ' : ''}$detail");
}

// 404 Not Found
class NotFoundException extends HttpException {
  const NotFoundException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.notFound,
            code: 'not_found',
            message: "Not Found${detail != '' ? ': ' : ''}$detail");
}

// 405 Method Not Allowed
class MethodNotAllowed extends HttpException {
  const MethodNotAllowed({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.methodNotAllowed,
            code: 'method_not_allowed',
            message: "Method Not Allowed${detail != '' ? ': ' : ''}$detail");
}

// 406 Not Acceptable
class NotAcceptableException extends HttpException {
  const NotAcceptableException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.notAcceptable,
            code: 'not_acceptable',
            message: "Not Acceptable${detail != '' ? ': ' : ''}$detail");
}

// 409 Conflict
class ConflictException extends HttpException {
  const ConflictException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.conflict,
            code: 'conflict',
            message: "Conflict${detail != '' ? ': ' : ''}$detail");
}

// 410 Gone
class GoneException extends HttpException {
  const GoneException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.gone,
            code: 'gone',
            message: "Gone${detail != '' ? ': ' : ''}$detail");
}

// 412 Precondition Failed
class PreconditionFailedException extends HttpException {
  const PreconditionFailedException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.preconditionFailed,
            code: 'precondition_failed',
            message: "Precondition Failed${detail != '' ? ': ' : ''}$detail");
}

// 415 Unsupported Media Type
class UnsupportedMediaTypeException extends HttpException {
  const UnsupportedMediaTypeException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.unsupportedMediaType,
            code: 'unsupported_media_type',
            message:
                "Unsupported Media Type${detail != '' ? ': ' : ''}$detail");
}

// 429 Too Many Requests
class TooManyRequestsException extends HttpException {
  const TooManyRequestsException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.tooManyRequests,
            code: 'too_many_requests',
            message: "Too Many Requests${detail != '' ? ': ' : ''}$detail");
}

/// 500 Internal Server Error
class InternalServerError extends HttpException {
  const InternalServerError({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.internalServerError,
            code: 'internal_server_error',
            message: "Internal Server Error${detail != '' ? ': ' : ''}$detail");
}

// 501 Not Implemented
class NotimplementedException extends HttpException {
  const NotimplementedException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.notImplemented,
            code: 'not_implemented',
            message: "Not Implemented${detail != '' ? ': ' : ''}$detail");
}

// 503 Service Unavailable
class ServiceUnavailableException extends HttpException {
  const ServiceUnavailableException({super.data, String detail = ''})
      : super(
            statusCode: io.HttpStatus.serviceUnavailable,
            code: 'service_unavailable',
            message: "Service Unavailable${detail != '' ? ': ' : ''}$detail");
}

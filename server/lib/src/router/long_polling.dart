import 'dart:async';

import 'package:shelf/shelf.dart' as shelf;
import 'package:ws_server/src/util/responses.dart';

FutureOr<shelf.Response> $longPolling(shelf.Request request) async {
  final requestTime = DateTime.now().toUtc();
  final ms = switch (request.url.queryParameters['duration']) {
    String value => int.tryParse(value) ?? 12000,
    _ => 12000,
  };
  await Future<void>.delayed(Duration(milliseconds: ms));
  final responseTime = DateTime.now().toUtc();
  return Responses.ok(<String, Object?>{
    'request': requestTime.toIso8601String(),
    'response': responseTime.toIso8601String(),
    'duration': responseTime.difference(requestTime).inMilliseconds.abs(),
  });
}

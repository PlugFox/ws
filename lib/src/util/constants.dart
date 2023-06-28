import 'package:meta/meta.dart';

/// Constants used to debug the WebSocket connection.
/// --dart-define=dev.plugfox.ws.debug=true
/// {@nodoc}
@internal
const bool $kDebugWS =
    bool.fromEnvironment('dev.plugfox.ws.debug', defaultValue: false);

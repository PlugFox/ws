import 'dart:async';

import 'package:meta/meta.dart';

/// Constants used to debug the WebSocket connection.
/// --dart-define=dev.plugfox.ws.debug=true
/// {@nodoc}
@internal
bool get $debugWS => _$kDebugWS || Zone.current[#dev.plugfox.ws.debug] == true;

/// {@nodoc}
const bool _$kDebugWS =
    bool.fromEnvironment('dev.plugfox.ws.debug', defaultValue: false);

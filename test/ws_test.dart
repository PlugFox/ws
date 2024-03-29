import 'package:test/test.dart';

import 'client/client_test.dart' as client_test;
import 'connection_manager/connection_manager_test.dart'
    as connection_manager_test;
import 'logger/logger_test.dart' as logger_test;
import 'message_stream/message_stream_test.dart' as message_stream_test;
import 'metrics/metrics_test.dart' as metrics_test;
import 'model/state_test.dart' as state_test;
import 'model/status_codes_test.dart' as status_codes_test;
import 'model/web_socket_ready_state_test.dart' as web_socket_ready_state_test;
import 'model/websocket_exception_test.dart' as websocket_exception_test;
import 'options/options_test.dart' as options_test;
import 'platform/platform_test.dart' as platform_test;
import 'states_stream/states_stream_test.dart' as states_stream_test;

void main() {
  group('WS tests', () {
    status_codes_test.main();
    web_socket_ready_state_test.main();
    websocket_exception_test.main();
    state_test.main();
    platform_test.main();
    client_test.main();
    metrics_test.main();
    message_stream_test.main();
    logger_test.main();
    connection_manager_test.main();
    options_test.main();
    states_stream_test.main();
  });
}

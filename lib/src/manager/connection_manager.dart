import 'dart:async';

import 'package:meta/meta.dart';
import 'package:ws/interface.dart';
import 'package:ws/src/util/logger.dart';

/// {@nodoc}
@internal
final class WebSocketConnectionManager {
  /// {@nodoc}
  static final WebSocketConnectionManager instance =
      WebSocketConnectionManager._internal();

  /// {@nodoc}
  WebSocketConnectionManager._internal();

  /// {@nodoc}
  final Expando<StreamSubscription<void>> _watchers =
      Expando<StreamSubscription<void>>();

  /// {@nodoc}
  final Expando<Timer> _timers = Expando<Timer>();

  /// Is reconnection enabled.
  /// {@nodoc}
  bool isReconnectionActive(IWebSocketClient client) =>
      _timers[client]?.isActive == true;

  /// {@nodoc}
  void startMonitoringConnection(IWebSocketClient client, String url) {
    stopMonitoringConnection(client);
    if (client.isClosed || client.reconnectTimeout <= Duration.zero) return;
    _watchers[client] = client.stateChanges.listen(
        (state) => _handleStateChange(client, url, state),
        cancelOnError: false);
  }

  /// {@nodoc}
  void stopMonitoringConnection(IWebSocketClient client) {
    _stopSubscription(client);
    _stopTimer(client);
  }

  /// {@nodoc}
  void _handleStateChange(
      IWebSocketClient client, String lastUrl, WebSocketClientState state) {
    switch (state) {
      case WebSocketClientState$Open _:
        _stopTimer(client);
        break;
      case WebSocketClientState$Closed _:
        _stopTimer(client);
        if (client.isClosed) return;
        _timers[client] = Timer.periodic(
          client.reconnectTimeout,
          (_) {
            if (client.isClosed) {
              _stopTimer(client);
            } else if (client.state.readyState.isClosed) {
              config('Auto reconnecting to $lastUrl '
                  'after ${client.reconnectTimeout.inSeconds} seconds');
              client.connect(lastUrl);
            }
          },
        );
        break;
      case WebSocketClientState$Connecting _:
      case WebSocketClientState$Disconnecting _:
        break;
    }
  }

  void _stopSubscription(IWebSocketClient client) {
    _watchers[client]?.cancel().ignore();
    _watchers[client] = null;
  }

  void _stopTimer(IWebSocketClient client) {
    _timers[client]?.cancel();
    _timers[client] = null;
  }
}

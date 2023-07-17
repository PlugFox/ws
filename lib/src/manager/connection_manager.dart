import 'dart:async';
import 'dart:math' as math;

import 'package:meta/meta.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/ws_client_interface.dart';
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

  /// {@nodoc}
  final Expando<int> _attempts = Expando<int>();

  /// {@nodoc}
  final Expando<DateTime> _nextReconnectionAttempts = Expando<DateTime>();

  /// Recive the current status of reconnection for the client.
  /// {@nodoc}
  ({
    int attempt,
    bool active,
    DateTime? nextReconnectionAttempt,
  }) getStatusFor(IWebSocketClient client) => (
        attempt: _attempts[client] ?? 0,
        active: _timers[client]?.isActive == true,
        nextReconnectionAttempt: _nextReconnectionAttempts[client],
      );

  /// {@nodoc}
  void startMonitoringConnection(
    IWebSocketClient client,
    String url,
    ({Duration max, Duration min})? connectionRetryInterval,
  ) {
    stopMonitoringConnection(client);
    if (client.isClosed || connectionRetryInterval == null) return;
    final stateChangesHandler = _handleStateChange(
      client,
      url,
      connectionRetryInterval.min.inMilliseconds,
      connectionRetryInterval.max.inMilliseconds,
    );
    _watchers[client] =
        client.stateChanges.listen(stateChangesHandler, cancelOnError: false);
  }

  /// {@nodoc}
  void stopMonitoringConnection(IWebSocketClient client) {
    _stopSubscription(client);
    _stopTimer(client);
  }

  /// {@nodoc}
  void Function(WebSocketClientState state) _handleStateChange(
    IWebSocketClient client,
    String lastUrl,
    int minMs,
    int maxMs,
  ) =>
      (state) {
        switch (state) {
          case WebSocketClientState$Open _:
            _stopTimer(client);
            _attempts[client] = null; // reset attempt
            _nextReconnectionAttempts[client] = null; // reset expected time
          case WebSocketClientState$Closed _:
            _stopTimer(client);
            if (client.isClosed) return;
            final attempt = _attempts[client] ?? 0;
            final delay = _backoffDelay(attempt, minMs, maxMs);
            if (delay <= Duration.zero) {
              config('Reconnecting to $lastUrl immediately.');
              Future<void>.sync(() => client.connect(lastUrl)).ignore();
              _attempts[client] = attempt + 1;
              return;
            }
            config('Reconnecting to $lastUrl '
                'after ${delay.inMilliseconds} ms.');
            _nextReconnectionAttempts[client] = DateTime.now().add(delay);
            _timers[client] = Timer(
              delay,
              () {
                _nextReconnectionAttempts[client] = null;
                if (client.isClosed) {
                  _stopTimer(client);
                } else if (client.state.readyState.isClosed) {
                  config('Auto reconnecting to $lastUrl '
                      'after ${delay.inMilliseconds} ms.');
                  Future<void>.sync(() => client.connect(lastUrl)).ignore();
                }
              },
            );
            _attempts[client] = attempt + 1;
          case WebSocketClientState$Connecting _:
          case WebSocketClientState$Disconnecting _:
        }
      };

  void _stopSubscription(IWebSocketClient client) {
    _watchers[client]?.cancel().ignore();
    _watchers[client] = null;
  }

  void _stopTimer(IWebSocketClient client) {
    _nextReconnectionAttempts[client] = null;
    _timers[client]?.cancel();
    _timers[client] = null;
  }

  /// Full jitter technique.
  /// https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/
  Duration _backoffDelay(int step, int minDelay, int maxDelay) {
    if (minDelay >= maxDelay) return Duration(milliseconds: maxDelay);
    final val = math.min(maxDelay, minDelay * math.pow(2, step.clamp(0, 31)));
    final interval = _rnd.nextInt(val.toInt());
    final milliseconds = math.min(maxDelay, minDelay + interval);
    return Duration(milliseconds: milliseconds);
  }

  /// Randomizer for full jitter technique.
  /// {@nodoc}
  static final math.Random _rnd = math.Random();
}

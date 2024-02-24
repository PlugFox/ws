import 'dart:async';
import 'dart:math' as math;

import 'package:meta/meta.dart';
import 'package:ws/src/client/state.dart';
import 'package:ws/src/client/ws_client_interface.dart';
import 'package:ws/src/util/logger.dart';

@internal
final class WebSocketConnectionManager {
  WebSocketConnectionManager(IWebSocketClient client)
      : _client = WeakReference<IWebSocketClient>(client);

  final WeakReference<IWebSocketClient> _client;

  StreamSubscription<void>? _watcher;

  Timer? _timer;

  int? _attempt;

  DateTime? _nextReconnectionAttempt;

  /// Recive the current status of reconnection for the client.
  ({
    int attempt,
    bool active,
    DateTime? nextReconnectionAttempt,
  }) get status => (
        attempt: _attempt ?? 0,
        active: _timer?.isActive == true,
        nextReconnectionAttempt: _nextReconnectionAttempt,
      );

  void startMonitoringConnection(
    String url,
    ({Duration max, Duration min})? connectionRetryInterval,
  ) {
    stopMonitoringConnection();
    final client = _client.target;
    if (client == null || client.isClosed || connectionRetryInterval == null) {
      return;
    }
    final stateChangesHandler = _handleStateChange(
      client,
      url,
      connectionRetryInterval.min.inMilliseconds,
      connectionRetryInterval.max.inMilliseconds,
    );
    _watcher =
        client.stateChanges.listen(stateChangesHandler, cancelOnError: false);
  }

  void stopMonitoringConnection() {
    _stopSubscription();
    _stopTimer();
  }

  void Function(WebSocketClientState state) _handleStateChange(
    IWebSocketClient client,
    String lastUrl,
    int minMs,
    int maxMs,
  ) =>
      (state) {
        switch (state) {
          case WebSocketClientState$Open _:
            _stopTimer();
            _attempt = null; // reset attempt
            _nextReconnectionAttempt = null; // reset expected time
          case WebSocketClientState$Closed _:
            _stopTimer();
            if (client.isClosed) return;
            final attempt = _attempt ?? 0;
            final delay = backoffDelay(attempt, minMs, maxMs);
            if (delay <= Duration.zero) {
              config('Reconnecting to $lastUrl immediately.');
              Future<void>.sync(() => client.connect(lastUrl)).ignore();
              _attempt = attempt + 1;
              return;
            }
            config('Reconnecting to $lastUrl '
                'after ${delay.inMilliseconds} ms.');
            _nextReconnectionAttempt = DateTime.now().add(delay);
            _timer = Timer(
              delay,
              () {
                _nextReconnectionAttempt = null;
                if (client.isClosed) {
                  _stopTimer();
                } else if (client.state.readyState.isClosed) {
                  config('Auto reconnecting to $lastUrl '
                      'after ${delay.inMilliseconds} ms.');
                  Future<void>.sync(() => client.connect(lastUrl)).ignore();
                }
              },
            );
            _attempt = attempt + 1;
          case WebSocketClientState$Connecting _:
          case WebSocketClientState$Disconnecting _:
        }
      };

  void _stopSubscription() {
    _watcher?.cancel().ignore();
    _watcher = null;
  }

  void _stopTimer() {
    _nextReconnectionAttempt = null;
    _timer?.cancel();
    _timer = null;
  }

  /// Full jitter technique.
  /// https://aws.amazon.com/blogs/architecture/exponential-backoff-and-jitter/
  @internal
  static Duration backoffDelay(int step, int minDelay, int maxDelay) {
    if (minDelay >= maxDelay) return Duration(milliseconds: maxDelay);
    final val = math.min(maxDelay, minDelay * math.pow(2, step.clamp(0, 31)));
    final interval = _rnd.nextInt(val.toInt());
    return Duration(milliseconds: math.min(maxDelay, minDelay + interval));
  }

  /// Randomizer for full jitter technique.
  static final math.Random _rnd = math.Random();
}

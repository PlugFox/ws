import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:ws/ws.dart';

/// Ties a [WebSocketClient] to the app lifecycle and, optionally, to
/// network changes.
///
/// ```dart
/// final client = WebSocketClient.json();
/// final lifecycle = WebSocketLifecycle(
///   client,
///   networkChanges: Connectivity().onConnectivityChanged, // connectivity_plus
/// );
/// await client.connect('wss://example.com/ws');
/// ...
/// lifecycle.dispose();
/// await client.close();
/// ```
///
/// - The app goes to the background ([AppLifecycleState.paused], Android and
///   iOS only): `client.suspend()`. The heartbeat and reconnects stop, so a
///   frozen or dozing app does not spend battery on a socket the OS is
///   about to kill. An open connection stays open unless
///   [disconnectInBackground].
/// - The app is visible and focused again ([AppLifecycleState.resumed] after
///   [AppLifecycleState.hidden] or [AppLifecycleState.paused]):
///   `client.resume()`. One ping checks the connection; a dead one is
///   replaced at once, without the reconnect backoff. Desktop and web report
///   `hidden` for a minimized window or a background tab and never
///   `paused`: their connection is not suspended, only checked on return
///   (browsers throttle timers in hidden tabs, so the heartbeat may be late).
/// - Each event of [networkChanges] while in the foreground:
///   `client.resume()`. A Wi-Fi to cellular switch leaves the old socket
///   bound to an address that no longer works, and nothing tells it so.
///
/// Calls of `connect()` or `reconnect()` while in the background end the
/// suspension (for example when a push message wakes the app to sync).
final class WebSocketLifecycle {
  /// Starts observing the app lifecycle for [client] and, if given,
  /// [networkChanges]. Call [dispose] when done; it does not close the
  /// client.
  WebSocketLifecycle(
    this.client, {
    this.disconnectInBackground = false,
    Stream<Object?>? networkChanges,
    WidgetsBinding? binding,
  }) {
    _listener = AppLifecycleListener(
      binding: binding,
      onStateChange: _onStateChange,
    );
    _network = networkChanges?.listen(
      (_) => _onNetworkChange(),
      // A failing source must not take the app down; it only stops checks.
      onError: (Object _, StackTrace _) {},
    );
  }

  /// The client this observes.
  final WebSocketClient<Object?> client;

  /// Whether `suspend()` also closes the connection (code 1000). Frees the
  /// server sooner; costs a full reconnect after every trip to another app.
  final bool disconnectInBackground;

  late final AppLifecycleListener _listener;
  StreamSubscription<Object?>? _network;

  /// The app went to the background and [client] was suspended.
  bool _suspended = false;

  /// The app was hidden or in the background since it was last resumed.
  bool _away = false;

  bool _disposed = false;

  void _onStateChange(AppLifecycleState state) {
    switch (state) {
      case AppLifecycleState.paused:
        _away = true;
        if (_suspended) return;
        _suspended = true;
        client.suspend(disconnect: disconnectInBackground);
      case AppLifecycleState.hidden:
        _away = true;
      case AppLifecycleState.resumed:
        if (!_away) return;
        _away = false;
        _suspended = false;
        _resume();
      case AppLifecycleState.inactive || AppLifecycleState.detached:
        break;
    }
  }

  void _onNetworkChange() {
    // resume() would end the suspension; the check happens on return.
    if (_suspended) return;
    _resume();
  }

  void _resume() {
    // Its result is the client's state; errors (disconnect() or close()
    // while waiting) are reported there.
    client.resume().ignore();
  }

  /// Stops observing. The client is left as it is, also when suspended.
  void dispose() {
    if (_disposed) return;
    _disposed = true;
    _listener.dispose();
    unawaited(_network?.cancel());
    _network = null;
  }
}

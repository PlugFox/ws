/// How many players are online, polled from `GET /api/online` (`{"online":
/// n, "server_time": unix ms, "build": client build}`): the title screens
/// show it with the request's round trip, the town beside the minimap; the
/// build tells an open tab of a new deploy (`update.dart`).
library;

import 'dart:async';

/// One poll's result.
final class OnlineStatus {
  const OnlineStatus({required this.online, this.pingMs, this.down = false, this.build});

  /// The server did not answer.
  const OnlineStatus.down() : online = 0, pingMs = null, down = true, build = null;

  /// The client build the server takes (`build`), when it says.
  final String? build;

  final int online;

  /// Round trip of the request (ms).
  final int? pingMs;
  final bool down;

  /// The engine's `ServerStatusSpec`.
  Map<String, Object?> toJson() => {'online': online, 'pingMs': ?pingMs, if (down) 'down': true};
}

/// Polls every [every] while started: once at [start], then on the timer.
/// A poll still in flight skips the next tick; answers that arrive after
/// [stop] are dropped.
final class OnlinePoller {
  OnlinePoller(this._fetch, this._onStatus, {this.every = const Duration(seconds: 15)});

  /// `GET /api/online` (decoded).
  final Future<Map<String, Object?>> Function() _fetch;
  final void Function(OnlineStatus status) _onStatus;
  final Duration every;

  Timer? _timer;
  bool _busy = false;

  /// The last answer, if any.
  OnlineStatus? last;

  bool get running => _timer != null;

  void start() {
    if (running) return;
    _timer = Timer.periodic(every, (_) => unawaited(poll()));
    unawaited(poll());
  }

  void stop() {
    _timer?.cancel();
    _timer = null;
  }

  /// One poll now (the timer's job; public for tests).
  Future<void> poll() async {
    if (_busy) return;
    _busy = true;
    final watch = Stopwatch()..start();
    OnlineStatus status;
    try {
      final r = await _fetch();
      status = OnlineStatus(
        online: (r['online'] as num?)?.toInt() ?? 0,
        pingMs: watch.elapsedMilliseconds,
        build: r['build'] as String?,
      );
    } on Object {
      status = const OnlineStatus.down();
    } finally {
      _busy = false;
    }
    if (!running) return;
    last = status;
    _onStatus(status);
  }
}

import 'protocol.dart';

/// The run's map events the HUD shows (server `hub/env.rs`, engine
/// `weather.ts`): which ones were warned of or are running, so their toast
/// can say what happened and so a new instance (back in town, a new stage,
/// a reconnect) takes every banner and edge warning they put up back down.
final class EnvEventLog {
  final _warned = <String>{};
  final _running = <String>{};

  /// Records [e] and returns its toast.
  String take(EnvEvent e) {
    switch (e.phase) {
      case EnvEvent.warning:
        _warned.add(e.id);
        return e.text.isNotEmpty ? e.text : '${e.name} is coming!';
      case EnvEvent.begins:
        _warned.remove(e.id);
        _running.add(e.id);
        return '${e.name}!';
      default:
        final begun = _running.remove(e.id);
        final warned = _warned.remove(e.id);
        // Warned of and never begun: the server called it off (the poison
        // fog closed the waves before the war horn sounded).
        return warned && !begun ? '${e.name} is called off.' : 'The ${e.name.toLowerCase()} is over.';
    }
  }

  /// Every event warned of or running (each to be ended on the view), and
  /// forgets them.
  List<String> clear() {
    final ids = {..._warned, ..._running}.toList();
    _warned.clear();
    _running.clear();
    return ids;
  }
}

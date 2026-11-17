import 'package:game_client/src/env_events.dart';
import 'package:game_client/src/protocol.dart';
import 'package:test/test.dart';

EnvEvent ev(int phase, {String id = 'war_horn', String text = ''}) =>
    EnvEvent(phase: phase, id: id, name: 'War Horn', text: text, startMs: 120000, durationMs: 25000);

void main() {
  test('a warning, its beginning and its end each have their toast', () {
    final log = EnvEventLog();
    expect(log.take(ev(EnvEvent.warning, text: 'A war horn sounds!')), 'A war horn sounds!');
    expect(log.take(ev(EnvEvent.begins)), 'War Horn!');
    expect(log.take(ev(EnvEvent.ends)), 'The war horn is over.');
    expect(log.clear(), isEmpty, reason: 'nothing left to take down');
  });

  test('an event warned of and ended unbegun was called off', () {
    final log = EnvEventLog();
    log.take(ev(EnvEvent.warning));
    expect(log.take(ev(EnvEvent.ends)), 'War Horn is called off.');
    expect(log.clear(), isEmpty);
  });

  test('a new instance takes down every event warned of or running, once', () {
    final log = EnvEventLog();
    log.take(ev(EnvEvent.warning));
    log.take(ev(EnvEvent.warning, id: 'sandstorm'));
    log.take(ev(EnvEvent.begins, id: 'sandstorm'));
    log.take(ev(EnvEvent.warning, id: 'flush_west'));
    log.take(ev(EnvEvent.ends, id: 'flush_west'));
    expect(log.clear()..sort(), ['sandstorm', 'war_horn']);
    expect(log.clear(), isEmpty);
  });
}

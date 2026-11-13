import 'dart:collection';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/interpolation.dart';
import 'package:game_client/src/prediction.dart';
import 'package:game_client/src/protocol.dart';
import 'package:game_client/src/walk_grid.dart';
import 'package:test/test.dart';

/// Same 12×7 town as the server's hub tests.
final town = WalkGrid.ascii([
  '############',
  '#..........#',
  '#..........#',
  '#..........#',
  '#..........#',
  '#..........#',
  '############',
]);

void main() {
  group('protocol', () {
    test('input frame layout matches the server decoder', () {
      final b = encodeInput(7, 1, -0.5);
      expect(b, hasLength(13));
      final d = ByteData.sublistView(b);
      expect(b[0], 0x02);
      expect(d.getUint32(1, Endian.little), 7);
      expect(d.getInt16(5, Endian.little), 32767);
      expect(d.getInt16(7, Endian.little), -16384);
    });

    test('snapshot decodes and rejects truncation', () {
      final d = ByteData(13 + 9)
        ..setUint8(0, 1)
        ..setUint16(1, 3, Endian.little)
        ..setUint32(3, 1234, Endian.little)
        ..setUint32(7, 55, Endian.little)
        ..setUint16(11, 1, Endian.little)
        ..setUint32(13, 42, Endian.little)
        ..setUint16(17, (12.5 * 64).round(), Endian.little)
        ..setUint16(19, (300.25 * 64).round(), Endian.little)
        ..setUint8(21, 6);
      final b = d.buffer.asUint8List();
      final s = Snapshot.decode(b)!;
      expect(
        (s.mapEpoch, s.tick, s.ackSeq, s.ids[0], s.xs[0], s.ys[0], s.facings[0]),
        (3, 1234, 55, 42, 12.5, 300.25, 6),
      );
      expect(s.mobBaseline, -1, reason: 'hub snapshots have no monster section');
      expect(Snapshot.decode(Uint8List.sublistView(b, 0, 21)), isNull);
    });

    // Golden frames: the same bytes are asserted by the server
    // (`world/protocol.rs`, `golden`). Change both together.
    Uint8List hex(String h) =>
        Uint8List.fromList([for (var i = 0; i < h.length; i += 2) int.parse(h.substring(i, i + 2), radix: 16)]);
    String toHex(Uint8List b) => b.map((v) => v.toRadixString(16).padLeft(2, '0')).join();

    test('client frames match the server goldens', () {
      expect(toHex(ClientMsg.hubJoin(2)), '4103');
      expect(toHex(ClientMsg.hubJoin()), '4100');
      expect(toHex(ClientMsg.missionSelect('m1', public: true)), '45026d3101');
      expect(toHex(ClientMsg.lobbyKick(300)), '4aac02');
      expect(toHex(ClientMsg.lobbyJoin('XY')), '46025859');
      expect(toHex(ClientMsg.resync()), '4c');
      expect(toHex(ClientMsg.styleSet(const Look(gender: 1, hair: 5, hairColor: 10, bodyColor: 6))), '4d01050a06');
      expect(toHex(encodeInput(9, 1, 0, aimX: -8, aimY: 16, slots: 33)), '0209000000ff7f0000f8102100');
      expect(toHex(ClientMsg.skillLearn('bolt')), '4e04626f6c74');
      expect(toHex(ClientMsg.skillSlot(4, 'slide')), '500405736c696465');
    });

    test('emote, pose and flag frames (protocol.rs header layout)', () {
      expect(toHex(ClientMsg.emote(46)), '512e');
      expect(toHex(ClientMsg.pose(sitting: true)), '5201');
      expect(toHex(ClientMsg.flag(3, 1.5, 2.25)), '530360009000');
      final e = ServerMsg.decode(hex('378080c003' '2e'))! as Emote;
      expect((e.entityId, e.emote), (0x700000, 46));
      final p = ServerMsg.decode(hex('380901'))! as Pose;
      expect((p.entityId, p.sitting), (9, true));
      final f = ServerMsg.decode(hex('390909' '6000' '9000'))! as FlagPing;
      expect((f.entityId, f.flag, f.x, f.y), (9, 9, 1.5, 2.25));
    });

    test('server goldens decode', () {
      final err = ServerMsg.decode(hex('280603666172'))! as HubError;
      expect((err.code, err.message), ('npc_too_far', 'far'));

      final joined = ServerMsg.decode(hex('30ac02016d14070206706f72696e6705666162726500002042'))! as StageJoined;
      expect(
        (joined.stageId, joined.mission, joined.tickRate, joined.entityId, joined.lightRadius),
        (300, 'm', 20, 7, 40.0),
      );
      expect(joined.monsters, ['poring', 'fabre']);

      final enter =
          ServerMsg.decode(
                hex(
                  '210166054669656c64029003020000003f00000034c200002041000020420000a04106612e6f676702006000900000010902426f066e6f76696365c000000106010402031f',
                ),
              )!
              as MapEnter;
      expect(
        (enter.map.id, enter.map.name, enter.map.kind, enter.map.width, enter.map.height),
        ('f', 'Field', 'field', 400, 2),
      );
      expect(enter.map.cellSize, 0.5);
      expect(enter.map.camera, {
        'type': 'free',
        'pitch': -45.0,
        'min_zoom': 10.0,
        'max_zoom': 40.0,
        'default_zoom': 20.0,
      });
      expect((enter.map.bgm, enter.mapEpoch, enter.x, enter.y, enter.viaPortal), ('a.ogg', 2, 1.5, 2.25, null));
      final p = enter.players.single;
      expect((p.entityId, p.name, p.archetype, p.x, p.y, p.facing), (9, 'Bo', 'novice', 3.0, 4.0, 6));
      expect(p.look, const Look(gender: 1, hair: 4, hairColor: 2, bodyColor: 3));
      expect(p.level, 31);

      final lobby =
          (ServerMsg.decode(
                    hex(
                      '310105414232434416706f72696e675f69736c616e645f737572766976616c16506f72696e672049736c616e6420537572766976616c48486f6c64206f757420616761696e73742067726f77696e67207761766573206f6e20506f72696e672049736c616e6420756e74696c2074686520626f73732073686f77732075702e0a7072745f66696c6430380400010902426f066e6f7669636503',
                    ),
                  )!
                  as LobbyState)
              .lobby;
      expect(
        (lobby.lobbyId, lobby.code, lobby.public, lobby.mission.id, lobby.mission.maxPlayers),
        (1, 'AB2CD', false, 'poring_island_survival', 4),
      );
      final m = lobby.members.single;
      expect((m.entityId, m.name, m.ready, m.leader), (9, 'Bo', true, true));

      expect(
        (ServerMsg.decode(hex('32ff06637573746f6d'))! as LobbyLeft).reason,
        'custom',
        reason: 'unknown codes travel as strings',
      );
      expect((ServerMsg.decode(hex('1105'))! as Pong).ts, -3);
      final look = ServerMsg.decode(hex('29ac02000c0f07'))! as PlayerLook;
      expect((look.entityId, look.look), (300, const Look(hair: 12, hairColor: 15, bodyColor: 7)));
      final v = ServerMsg.decode(hex('2aac02287a07830101800503041f'))! as PlayerVitals;
      expect((v.entityId, v.hp, v.maxHp, v.sp, v.maxSp, v.state), (300, 40, 122, 7, 131, 1));
      expect((v.status, v.tempHp, v.light, v.level), (640, 3, 4, 31));
      final c = ServerMsg.decode(hex('2b01ac02060287808004c80115ac020002'))! as Combat;
      expect((c.swings.single.entityId, c.swings.single.facing), (300, 6));
      final kill = c.hits[0], miss = c.hits[1];
      expect((kill.target, kill.amount, kill.isCritical, kill.isKill, kill.damageType), (monsterEntityBase + 7, 200, true, true, 1));
      expect((miss.target, miss.isMiss, miss.amount), (300, true, 0));
      final pr = ServerMsg.decode(hex('2c1fd8a705e8f505e102'))! as Progress;
      expect((pr.level, pr.exp, pr.expToNext, pr.gained), (31, 87000, 97000, 353));
      final sk = ServerMsg.decode(hex('2d0c030104626f6c74040204626f6c7400'))! as Skills;
      expect((sk.level, sk.points), (12, 3));
      expect(sk.levels, {'bolt': 4});
      expect(sk.hotbar, ['bolt', '']);
      final cast = ServerMsg.decode(hex('2eac0204626f6c74a002d0009c04e80701090102a002d0004000024000f02eac0203'))! as Cast;
      expect((cast.caster, cast.skill, cast.x, cast.y, cast.castMs, cast.cooldownMs), (300, 'bolt', 10.5, 3.25, 540, 1000));
      expect(cast.level, 3);
      final t = cast.areas.single;
      expect((t.id, t.shape, t.look, t.x, t.y, t.dir, t.a, t.b, t.spanMs, t.follow), (9, 1, 2, 10.5, 3.25, 64, 8.0, 1.0, 6000, 300));
      expect((ServerMsg.decode(hex('2f09'))! as ZoneEnd).id, 9);
      final end = ServerMsg.decode(hex('3401a503d209cd8306904e'))! as StageEnd;
      expect((end.victory, end.secs, end.kills, end.exp, end.leaveMs), (true, 421, 1234, 98765, 10000));
      final boss = ServerMsg.decode(hex('35858080040442617068d00fe615'))! as Boss;
      expect((boss.entityId, boss.name, boss.hp, boss.maxHp), (monsterEntityBase + 5, 'Baph', 2000, 2790));
      final buffs = ServerMsg.decode(hex('36010568617374659423'))! as Buffs;
      expect(buffs.buffs.single, (skill: 'haste', ms: 4500));
    });

    test('looks come from the REST character JSON', () {
      expect(
        Look.fromJson({'gender': 1, 'hair_style': 7, 'hair_color': 3, 'body_color': 2}),
        const Look(gender: 1, hair: 7, hairColor: 3, bodyColor: 2),
      );
      expect(Look.fromJson(null), const Look(), reason: 'older servers: default look');
    });

    test('unknown kinds are ignored, malformed known ones throw', () {
      expect(ServerMsg.decode(hex('7f0102')), isNull);
      expect(() => ServerMsg.decode(hex('2806')), throwsA(isA<WireFormatError>()));
      expect(() => ServerMsg.decode(hex('110500')), throwsA(isA<WireFormatError>()), reason: 'trailing bytes');
    });

    test('stage snapshot golden carries every monster op', () {
      final s = Snapshot.decode(
        hex('030400640000000c000000010009000000600080000201041a05018002000510f00402fe0300809902ffff00007f0103'),
      )!;
      expect((s.mapEpoch, s.tick, s.ackSeq, s.length, s.ids[0], s.xs[0], s.ys[0]), (4, 100, 12, 1, 9, 1.5, 2.0));
      expect(s.mobBaseline, 1);
      expect(s.mobOps, [MobOp.spawn, MobOp.nudge, MobOp.move, MobOp.remove]);
      expect(s.mobSlots, [3, 4, 40, 41]);
      expect(
        (s.mobGenerations[0], s.mobKinds[0], s.mobXs[0], s.mobYs[0], s.mobVxs[0], s.mobVys[0]),
        (5, 1, 640, 1280, 16, -16),
      );
      expect((s.mobXs[1], s.mobYs[1], s.mobVxs[1], s.mobVys[1]), (-2, 3, 0, -128));
      expect((s.mobXs[2], s.mobYs[2], s.mobVxs[2], s.mobVys[2]), (65535, 0, 127, 1));
      expect(s.mobStatuses, [-1, MobStatus.frozen, -1, -1], reason: 'the NUDGE carries a status byte');
      final poisoned = Snapshot.decode(
        hex('030400640000000c000000010009000000600080000201041a05018002000510f0048001fe030080' '9902ffff00007f0103'),
      )!;
      expect(poisoned.mobStatuses[1], MobStatus.poisoned, reason: 'the status is a varint');
      expect(poisoned.mobXs[1], -2);
      final b = hex('030400640000000c000000010009000000600080000201041a05018002000510f00402fe0300809902ffff00007f0103');
      expect(Snapshot.decode(Uint8List.sublistView(b, 0, b.length - 1)), isNull);
      expect(Snapshot.decode(Uint8List.fromList([...b, 0])), isNull, reason: 'trailing bytes');
    });

    test('facing matches the server convention', () {
      expect(facingOf(0, 1), 0);
      expect(facingOf(1, 0), 2);
      expect(facingOf(0, -1), 4);
      expect(facingOf(-1, 0), 6);
      expect(facingOf(1, 1), 1);
    });
  });

  group('prediction', () {
    test('move rules slide along walls like the server', () {
      const rules = MoveRules();
      var (x, y) = (10.5, 5.5);
      for (var i = 0; i < 10; i++) {
        (x, y) = rules.apply(town, x, y, 1, 1);
      }
      expect(x, lessThan(11), reason: 'east wall at x = 11');
      expect(y, lessThan(6), reason: 'north wall at y = 6');
      expect(x, greaterThan(10.5));
    });

    test('reconciling against a lagging server leaves no error', () {
      // Server applies each input `lag` ticks after the client predicted it.
      const lag = 3;
      final p = LocalPredictor(town, 2.5, 3.5);
      var (sx, sy) = (2.5, 3.5);
      final inFlight = Queue<(int, double, double)>();
      for (var t = 0; t < 40; t++) {
        final dir = t < 25 ? (1.0, 0.3) : (0.0, 0.0);
        final seq = p.step(dir.$1, dir.$2);
        inFlight.add((seq, dir.$1, dir.$2));
        if (inFlight.length > lag) {
          final (s, dx, dy) = inFlight.removeFirst();
          (sx, sy) = const MoveRules().apply(town, sx, sy, dx, dy);
          p.reconcile(s, sx, sy);
        }
        expect(p.errX.abs() + p.errY.abs(), lessThan(1e-9), reason: 'tick $t');
      }
    });

    test('a server disagreement is smoothed, not snapped', () {
      final p = LocalPredictor(town, 2.5, 3.5);
      for (var i = 0; i < 5; i++) {
        p.step(1, 0);
      }
      final (before, _) = p.render(0.5, 0);
      // The server says we only moved four steps (it dropped one input).
      p.reconcile(5, 2.5 + 4 * 0.3, 3.5);
      final (after, _) = p.render(0.5, 0);
      expect(after, closeTo(before, 1e-9), reason: 'no visible jump at the correction');
      expect(p.x, closeTo(3.7, 1e-6));
      for (var i = 0; i < 30; i++) {
        p.render(1, 1 / 60);
      }
      expect(p.errX.abs(), lessThan(0.01), reason: 'error decays');
    });

    test('large corrections snap (teleport)', () {
      final p = LocalPredictor(town, 2.5, 3.5)..step(1, 0);
      p.reconcile(1, 9.5, 1.5);
      expect(p.render(1, 0), (9.5, 1.5));
    });
  });

  group('interpolation', () {
    test('samples between snapshots and holds after extrapolating', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 2)
        ..push(11, 0.3, 0, 2)
        ..push(12, 0.6, 0, 2);
      t.sample(11.5);
      expect(t.x, closeTo(0.45, 1e-9));
      expect(t.moving, isTrue);
      t.sample(13);
      expect(t.x, closeTo(0.9, 1e-9), reason: 'one tick of extrapolation');
      t.sample(30);
      expect(t.x, closeTo(1.2, 1e-9), reason: 'capped at two ticks');
      expect(t.moving, isFalse);
    });

    test('segment brackets the render tick and keeps the last pair past it', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 2)
        ..push(11, 0.3, 0, 2)
        ..push(12, 0.6, 0, 3);
      t.segment(11.5);
      expect((t.tick0, t.tick1, t.x0, t.x1), (11, 12, 0.3, 0.6));
      expect(t.facing, 3);
      expect(t.moving, isTrue);
      t.segment(12.5);
      expect((t.tick0, t.tick1), (11, 12), reason: 'the shader extrapolates along the last pair');
      expect(t.moving, isTrue);
      t.segment(30);
      expect(t.moving, isFalse, reason: 'stops walking once extrapolation is exhausted');
      t.segment(5);
      expect((t.x0, t.x1, t.tick0, t.tick1), (0.0, 0.0, 10, 10), reason: 'before the oldest sample: a point');
    });

    test('segment of a teleport is a point at the destination', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 0)
        ..push(11, 50, 50, 0);
      t.segment(10.5);
      expect((t.x0, t.y0, t.x1, t.y1), (50.0, 50.0, 50.0, 50.0));
      expect(t.moving, isFalse);
    });

    test('segment without samples stands at the spawn position', () {
      final t = RemoteTrack(1)
        ..x = 4
        ..y = 7;
      t.segment(0);
      expect((t.x0, t.y0, t.x1, t.y1), (4.0, 7.0, 4.0, 7.0));
    });

    test('teleports are not blended', () {
      final t = RemoteTrack(1)
        ..push(10, 0, 0, 0)
        ..push(11, 50, 50, 0);
      t.sample(10.5);
      expect((t.x, t.y), (50.0, 50.0));
    });

    test('render delay adapts to jitter', () {
      final calm = ServerClock(tickHz: 20);
      final jittery = ServerClock(tickHz: 20);
      final rnd = math.Random(1);
      for (var tick = 0; tick < 400; tick++) {
        calm.onSnapshot(tick, tick / 20 + 0.05);
        jittery.onSnapshot(tick, tick / 20 + 0.05 + rnd.nextDouble() * 0.06);
      }
      expect(calm.delay, closeTo(0.075, 0.01));
      expect(jittery.delay, greaterThan(calm.delay + 0.04));
      // Rendering stays behind the newest tick by about the delay.
      final now = 399 / 20 + 0.05;
      expect(399 - calm.renderTick(now), closeTo(calm.delay * 20, 0.2));
    });
  });
}

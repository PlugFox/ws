import 'dart:collection';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:game_client/src/classes.dart';
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
      expect(toHex(ClientMsg.shopBuy('dega', 'sword', 2)), '5404646567610573776f726402');
      expect(toHex(ClientMsg.shopSell('dega', 'sword', 1)), '5504646567610573776f726401');
      expect(toHex(ClientMsg.equip('clip')), '5604636c6970ff');
      expect(toHex(ClientMsg.unequip(EquipSlot.shield)), '5705');
      expect(toHex(ClientMsg.itemUse('red_potion')), '580a7265645f706f74696f6e');
    });

    test('mission start golden (protocol.rs `mission_start_matches_the_dart_golden`)', () {
      expect(toHex(ClientMsg.missionStart('m1', tier: 2, mask: 65)), '63026d310241');
    });

    test('cast prefs golden (protocol.rs `cast_prefs_match_the_dart_golden`)', () {
      expect(toHex(ClientMsg.castPrefs(manualAim: true, auto: 0x8009)), '85010980');
      expect(toHex(ClientMsg.castPrefs(manualAim: false, auto: 0)), '85000000');
    });

    test('item goldens decode', () {
      final inv =
          ServerMsg.decode(
                hex(
                  '3aac02010a7265645f706f74696f6e050000000c636f74746f6e5f73686972740573776f72640000000000097b226163223a31327d',
                ),
              )!
              as Inventory;
      expect(inv.zeny, 300);
      expect(inv.items.single, (id: 'red_potion', count: 5));
      expect(inv.count('red_potion'), 5);
      expect(inv.equip, ['', '', '', 'cotton_shirt', 'sword', '', '', '', '', '']);
      expect(inv.statsJson, '{"ac":12}');
      final defs = ServerMsg.decode(hex('3b0103636170034361700500a01f0b052b31204143'))! as ItemDefs;
      final cap = defs.items.single;
      expect(
        (cap.id, cap.name, cap.kind, cap.slot, cap.price, cap.classes, cap.desc),
        ('cap', 'Cap', 5, 0, 4000, 11, '+1 AC'),
      );
      final shop = ServerMsg.decode(hex('3c04646567610444656761010573776f7264'))! as Shop;
      expect((shop.npc, shop.name), ('dega', 'Dega'));
      expect(shop.items, ['sword']);
      final used = ServerMsg.decode(hex('3d0a7265645f706f74696f6ee807'))! as ItemUsed;
      expect((used.item, used.cooldownMs), ('red_potion', 1000));
      final gear = ServerMsg.decode(hex('3eac0203626f77'))! as PlayerGear;
      final hats = ServerMsg.decode(hex('8bac02020368617408666c755f6d61736b'))! as PlayerHeadgear;
      expect(hats.entityId, 300);
      expect(hats.items, ['hat', 'flu_mask']);
      expect((gear.entityId, gear.weapon), (300, 'bow'));
      final loot = ServerMsg.decode(hex('3f85808004a002d000056b6e6966650101'))! as Loot;
      expect(
        (loot.entityId, loot.x, loot.y, loot.item, loot.count, loot.rare, loot.isZeny),
        (0x800005, 10.5, 3.25, 'knife', 1, true, false),
      );
    });

    test('mission list golden decodes (protocol/mission.rs `MISSION_LIST`)', () {
      final l = ServerMsg.decode(
        hex(
          '9201016d014d01640173016601500764756e67656f6e01610f121e0f238407080101420103666f6700017201017801',
        ),
      )! as MissionList;
      final c = l.missions.single;
      expect((c.id, c.name, c.description, c.story, c.map, c.place, c.terrain, c.art), ('m', 'M', 'd', 's', 'f', 'P', 'dungeon', 'a'));
      expect((c.band.min, c.band.recommended, c.band.max, c.monsterLevels, c.timeLimitSecs, c.maxPlayers), (15, 18, 30, (15, 35), 900, 8));
      expect([c.bosses, c.tags, c.tips], [['B'], ['fog'], <String>[]]);
      expect(c.rewards, 'r');
      expect((c.soon, c.reason, c.step), (true, 'x', 1));
    });

    test('combo golden decodes (protocol.rs `combo`)', () {
      final c = ServerMsg.decode(hex('60bb0105b8178617'))! as Combo;
      expect((c.count, c.rank, c.rankName, c.windowMs, c.leftMs), (187, 5, 'S', 3000, 2950));
      final dropped = ServerMsg.decode(hex('6000000000'))! as Combo;
      expect((dropped.count, dropped.rankName), (0, ''));
    });

    test('revive golden decodes (protocol.rs `revive`)', () {
      final r = ServerMsg.decode(hex('61ac022de28601'))! as Revive;
      expect((r.entityId, r.progress, r.leftMs, r.isDown), (300, 45, 17250, true));
      expect(ServerMsg.decode(hex('61ac020000'))! as Revive, isA<Revive>().having((r) => r.isDown, 'isDown', false));
    });

    test('bonus goldens decode (protocol.rs `bonus`, `bonus_gone`)', () {
      final b = ServerMsg.decode(hex('680007047a656e79a002d000b0ea01'))! as BonusSpawn;
      expect((b.id, b.bonus, b.x, b.y, b.lifeMs), (7, 'zeny', 10.5, 3.25, 30000));
      final g = ServerMsg.decode(hex('680107047a656e79ac02'))! as BonusGone;
      expect((g.id, g.bonus, g.entityId, g.expired), (7, 'zeny', 300, false));
      expect((ServerMsg.decode(hex('6801070000'))! as BonusGone).expired, true);
      expect(() => ServerMsg.decode(hex('6802')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('mission_info and stage_clock goldens decode (protocol.rs `golden_brief`)', () {
      final b =
          (ServerMsg.decode(
                    hex(
                      '6c016d0166840701145a010170015005737761726d05776174657205736d616c6c00010a0515060c01056170706c65ac02010201030fbc0501047469646504546964650278a4030f28',
                    ),
                  )!
                  as MissionBriefing)
              .brief;
      expect((b.mission, b.map, b.timeLimitSecs, b.levelMin, b.levelMax, b.levelEverySecs), ('m', 'f', 900, 1, 20, 90));
      final m = b.monsters.single;
      expect(
        (
          m.id,
          m.name,
          m.archetype,
          m.element,
          m.size,
          m.unlockSecs,
          m.levelMin,
          m.levelMax,
          m.hpMin,
          m.hpMax,
          m.packMin,
          m.packMax,
        ),
        ('p', 'P', 'swarm', 'water', 'small', 0, 1, 10, 5, 21, 6, 12),
      );
      expect(m.drops.single, (item: 'apple', rate: 300, min: 1, max: 2));
      expect(b.toJson()['levels'], [1, 20]);
      // Mission platform fields (trailing).
      expect((b.band, b.lightPermille), ((min: 1, recommended: 3, max: 15), 700));
      final ev = b.events.single;
      expect((ev.id, ev.name, ev.warnSecs, ev.durationSecs), ('tide', 'Tide', 15, 40));
      expect(ev.starts, [120, 420]);
      expect(b.underLevel([1]), false);
      // An older server's brief ends after the monsters.
      final old = (ServerMsg.decode(hex('6c016d0166840701145a00'))! as MissionBriefing).brief;
      expect((old.band.min, old.lightPermille, old.events.length), (1, 1000, 0));
      expect((ServerMsg.decode(hex('6d84c305'))! as StageClock).elapsedMs, 90500);
    });

    test('mission platform goldens decode (protocol/mission.rs `frames`)', () {
      final l = ServerMsg.decode(hex('93000000e041dc0b'))! as EnvLight;
      expect((l.radius, l.fadeMs), (28.0, 1500));
      final e = ServerMsg.decode(hex('9301000474696465045469646503557021c0a907c0b802'))! as EnvEvent;
      expect((e.phase, e.id, e.name, e.text, e.startMs, e.durationMs), (EnvEvent.warning, 'tide', 'Tide', 'Up!', 120000, 40000));
      final k = ServerMsg.decode(hex('9302c8a006'))! as EnvShake;
      expect((k.intensity, k.durationMs), (200, 800));
      expect(() => ServerMsg.decode(hex('9309')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('fog goldens decode (protocol.rs `fog`, `fog_exposure`)', () {
      final f = ServerMsg.decode(hex('6e0080f92b80f92bf09309a002d000000a8002'))! as PoisonFog;
      expect(
        (f.begun, f.nowMs, f.startMs, f.shrinkMs, f.x, f.y, f.fromRadius, f.toRadius),
        (true, 720000, 720000, 150000, 10.5, 3.25, 40.0, 10.0),
      );
      expect(f.radiusAt(700000), 40.0);
      expect(f.radiusAt(795000), 25.0);
      expect(f.radiusAt(900000), 10.0);
      final e = ServerMsg.decode(hex('6e020c1e'))! as FogExposure;
      expect((e.secs, e.deathSecs, e.fatal), (12, 30, false));
      expect(() => ServerMsg.decode(hex('6e03')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('boss goldens (protocol/boss.rs `golden::boss`)', () {
      final c = ServerMsg.decode(hex('6a858080040b626f73735f70726179657206507261796572942301'))! as BossCast;
      expect(
        (c.caster, c.ability, c.name, c.castMs, c.channel, c.interrupted),
        (0x800005, 'boss_prayer', 'Prayer', 4500, true, false),
      );
      final z =
          ServerMsg.decode(
                hex(
                  '6b8580800409626f73735f68616c6f02000307a002d000008001a0000000c00c000000040003000100800000000000c00c01',
                ),
              )!
              as BossZones;
      expect((z.caster, z.ability, z.zones.length), (0x800005, 'boss_halo', 2));
      final ring = z.zones[0];
      expect(
        (ring.area.shape, ring.area.look, ring.area.x, ring.area.y, ring.area.a, ring.area.b),
        (3, 7, 10.5, 3.25, 6.0, 2.5),
      );
      expect((ring.delayMs, ring.safe), (1600, false));
      expect((z.zones[1].area.shape, z.zones[1].area.a, z.zones[1].safe), (0, 2.0, true));
      expect(z.zones[1].toJson()['delayMs'], 1600);
      final s = ServerMsg.decode(hex('6f8580800408416e67656c696e67064b6e65656c2101'))! as BossShout;
      expect((s.entityId, s.name, s.text, s.style), (0x800005, 'Angeling', 'Kneel!', 1));
      final b = ServerMsg.decode(hex('728580800408416e67656c696e6702d00fe615ac02010346280f01'))! as BossBar;
      expect(
        (b.entityId, b.name, b.rank, b.hp, b.maxHp, b.shield, b.phase, b.enraged),
        (0x800005, 'Angeling', 2, 2000, 2790, 300, 1, true),
      );
      expect(b.pips, [70, 40, 15]);
    });

    test('chat goldens (protocol.rs `chat`, `announce`, `chat_req`)', () {
      expect(toHex(ClientMsg.chat('hi')), '5e026869');
      final c = ServerMsg.decode(hex('7a00ac0203416e6e03686921'))! as ChatLine;
      expect((c.entityId, c.name, c.text), (300, 'Ann', 'hi!'));
      final a = ServerMsg.decode(hex('7a0103627965'))! as Announcement;
      expect(a.text, 'bye');
      expect(() => ServerMsg.decode(hex('7a02')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('party and inspect goldens (protocol.rs `party_*`, `player_inspect`, `inspect_req`)', () {
      expect(toHex(ClientMsg.partyInvite(300)), '5900ac02');
      expect(toHex(ClientMsg.partyAnswer(300, accept: true)), '5901ac0201');
      expect(toHex(ClientMsg.inspect(300)), '5aac02');
      final i = ServerMsg.decode(hex('7000ac0203416e6e014db0ea01'))! as PartyInvite;
      expect((i.from, i.name, i.mission, i.ttlMs), (300, 'Ann', 'M', 30000));
      final a = ServerMsg.decode(hex('7001ac0203416e6e01'))! as PartyAnswer;
      expect((a.entityId, a.name, a.accepted), (300, 'Ann', true));
      final p =
          ServerMsg.decode(hex('71ac0203416e6e066e6f766963650c01050a06000000000573776f72640000000000'))!
              as PlayerInspect;
      expect((p.entityId, p.name, p.archetype, p.level), (300, 'Ann', 'novice', 12));
      expect((p.look.gender, p.look.hair, p.look.hairColor, p.look.bodyColor), (1, 5, 10, 6));
      expect(p.equip, ['', '', '', '', 'sword', '', '', '', '', '']);
      expect(() => ServerMsg.decode(hex('7002')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('friends goldens (protocol.rs `golden::friends`)', () {
      expect(toHex(ClientMsg.friendList()), '5d00');
      expect(toHex(ClientMsg.friendAdd(300)), '5d01ac02');
      expect(toHex(ClientMsg.friendAddName('Ann')), '5d0203416e6e');
      expect(toHex(ClientMsg.friendAccept(7)), '5d0307');
      expect(toHex(ClientMsg.friendDecline(7)), '5d0407');
      expect(toHex(ClientMsg.friendRemove(7)), '5d0507');
      final l =
          ServerMsg.decode(
                hex(
                  '7802070003416e6e01046d6167650c0850726f6e746572610202ac02ad02090103426f6200066172636865720300000105',
                ),
              )!
              as Friends;
      final [ann, bob] = l.friends;
      expect(
        (ann.account, ann.link, ann.name, ann.online, ann.archetype, ann.level),
        (7, FriendLink.friend, 'Ann', true, 'mage', 12),
      );
      expect((ann.map, ann.channel), ('Prontera', 2));
      expect(ann.characters, [300, 301]);
      expect((bob.account, bob.link, bob.online, bob.map, bob.channel), (9, FriendLink.incoming, false, '', 0));
      expect(bob.characters, [5]);
      final n = ServerMsg.decode(hex('79000703416e6e'))! as FriendNote;
      expect((n.op, n.account, n.name), (FriendNoteOp.online, 7, 'Ann'));
      expect(() => ServerMsg.decode(hex('7801070903416e6e')), throwsA(isA<WireFormatError>()), reason: 'bad link');
    });

    test('trade goldens (protocol.rs `golden::trade`)', () {
      expect(toHex(ClientMsg.tradeRequest(300)), '5b00ac02');
      expect(toHex(ClientMsg.tradeRespond(accept: true)), '5b01');
      expect(toHex(ClientMsg.tradeRespond(accept: false)), '5b02');
      expect(toHex(ClientMsg.tradeCancel()), '5b03');
      expect(toHex(ClientMsg.tradeLock()), '5b04');
      expect(toHex(ClientMsg.tradeConfirm()), '5b05');
      expect(toHex(ClientMsg.tradeZeny(1500)), '5b06dc0b');
      expect(toHex(ClientMsg.tradeItem('red_potion', 3)), '5b070a7265645f706f74696f6e03');
      final q = ServerMsg.decode(hex('7400ac0203416e6e0c'))! as TradeRequest;
      expect((q.entityId, q.name, q.level), (300, 'Ann', 12));
      final s =
          ServerMsg.decode(hex('7401ac0203416e6edc0b020a7265645f706f74696f6e030573776f726401000006'))! as TradeState;
      expect((s.entityId, s.name, s.mine.zeny, s.theirs.zeny, s.theirs.items.length), (300, 'Ann', 1500, 0, 0));
      expect(s.mine.items, [(id: 'red_potion', count: 3), (id: 'sword', count: 1)]);
      expect((s.myLock, s.theirLock, s.myConfirm, s.theirConfirm), (false, true, true, false));
      final c = ServerMsg.decode(hex('7402ff04646f6e650f547261646520636f6d706c6574652e'))! as TradeClosed;
      expect((c.reason, c.message), ('done', 'Trade complete.'));
      expect(() => ServerMsg.decode(hex('7403')), throwsA(isA<WireFormatError>()), reason: 'unknown op');
    });

    test('shop goldens (protocol.rs `golden::shop`)', () {
      expect(
        toHex(
          ClientMsg.shopDeal('tool', [(id: 'red_potion', count: 12)], [(id: 'jellopy', count: 300), (id: 'sword+7', count: 1)]),
        ),
        '8c04746f6f6c010a7265645f706f74696f6e0c02076a656c6c6f7079ac020773776f72642b3701',
      );
      final d = ServerMsg.decode(hex('8dd804e209'))! as ShopDone;
      expect((d.spent, d.earned), (600, 1250));
    });

    test('feedback goldens (protocol.rs `golden::feedback`)', () {
      expect(
        toHex(
          ClientMsg.feedback(
            kind: 'bug',
            priority: 'high',
            title: 'Stuck',
            body: 'Line one\nLine two',
            clientVersion: '0.27',
            userAgent: 'UA',
          ),
        ),
        '5c03627567046869676805537475636b114c696e65206f6e650a4c696e652074776f04302e3237025541',
      );
      final c = ServerMsg.decode(hex('10090000000000000000000000000000123408302e312e302b6162'))! as Connected;
      expect((c.userId, c.serverVersion), (9, '0.1.0+ab'));
      final old = ServerMsg.decode(hex('100900000000000000000000000000001234'))! as Connected;
      expect(old.serverVersion, '', reason: 'older servers send no version');
      final ok = ServerMsg.decode(hex('62ac0200075468616e6b7321'))! as FeedbackResult;
      expect((ok.id, ok.code, ok.message, ok.ok), (300, '', 'Thanks!', true));
      final no = ServerMsg.decode(hex('620011666565646261636b5f746f6f5f736f6f6e05576169742e'))! as FeedbackResult;
      expect((no.id, no.code, no.message, no.ok), (0, 'feedback_too_soon', 'Wait.', false));
    });

    test('link goldens (protocol.rs `link_frames_round_trip_and_match_the_dart_goldens`)', () {
      final l = ServerMsg.decode(hex('73ac0200'))! as PlayerLink;
      expect((l.entityId, l.online), (300, false));
      final r = ServerMsg.decode(hex('750706506f72696e67c8df02'))! as Resumable;
      expect((r.stageId, r.mission, r.leftMs), (7, 'Poring', 45000));
      expect(toHex(ClientMsg.stageResume(accept: true)), '5f01');
      expect(toHex(ClientMsg.stageResume(accept: false)), '5f00');
    });

    test('monster hp golden (protocol.rs `monster_hp_matches_the_dart_golden`)', () {
      final m = ServerMsg.decode(hex('a50287808004ffac82800400'))! as MonsterHp;
      expect(m.bars, [(monsterEntityBase + 7, 255), (monsterEntityBase + 300, 0)]);
      expect(() => ServerMsg.decode(hex('a50287808004ffac828004')), throwsA(isA<WireFormatError>()));
    });

    test('mount goldens (protocol.rs `mount_frames_match_the_dart_goldens`)', () {
      final m = ServerMsg.decode(hex('90ac020119'))! as PlayerMount;
      expect((m.entityId, m.mount, m.speed, m.riding), (300, PlayerMount.peco, 25, true));
      expect(toHex(ClientMsg.mount(on: false)), '9100');
      expect(toHex(ClientMsg.mount(on: true)), '9101');
      expect(riderJob('swordsman', 2, riding: true), 'crusader_peco');
      expect(riderJob('swordsman', 3, riding: true), 'paladin_peco');
      expect(riderJob('swordsman', 1, riding: true), 'swordsman', reason: 'no mounted swordsman body');
      expect(riderJob('swordsman', 2, riding: false), 'crusader');
    });

    test('modifier goldens (protocol/modifiers.rs `goldens`)', () {
      final d = ServerMsg.decode(
        hex(
          '7b020a646966666963756c7479ff01440164016904000001014e017804736c6f770101530173016a060a010002000000c8016405',
        ),
      )! as ModifierDefs;
      expect(d.defs.map((x) => (x.id, x.bit, x.risk, x.minLevel, x.locked)), [
        ('difficulty', ModifierDef.slider, 4, 0, false),
        ('slow', 1, 6, 10, true),
      ]);
      expect(d.defs[0].steps, [('N', 'x')]);
      expect(d.defs[1].toJson()['steps'], isEmpty);
      expect(d.rewards.map((r) => (r.zenyPct, r.expPct, r.chest)), [(0, 0, 0), (200, 100, 5)]);
      final m = ServerMsg.decode(hex('7c00010342183018013b'))! as ModifierState;
      expect((m.scope, m.tier, m.mask, m.risk, m.zenyPct, m.expPct, m.chest, m.lootPct), (1, 3, 0x42, 24, 48, 24, 1, 59));
      final r = ServerMsg.decode(hex('7c01030212ac02dc0b01076a656c6c6f707902'))! as ModifierReward;
      expect((r.tier, r.mask, r.risk, r.zeny, r.exp), (3, 2, 18, 300, 1500));
      expect(r.items, [('jellopy', 2)]);
      expect(toHex(ClientMsg.lobbyModifiers(3, 0x42)), '770342');
    });

    test('affix goldens (protocol/affixes.rs `goldens`)', () {
      final m = ServerMsg.decode(hex('a00205021100ac02ff80800205'))! as MonsterAffixes;
      expect(m.monsters.map((a) => (a.slot, a.generation, a.bits, a.state)), [(5, 2, 0x11, 0), (300, 255, 0x8000, 5)]);
      expect((m.monsters[1].frenzied, m.monsters[1].minion, m.monsters[1].shieldBroken), (true, true, false));
      final e = ServerMsg.decode(hex('a1018580800401a002d000'))! as AffixEvents;
      expect((e.events.single.entity, e.events.single.name, e.events.single.x, e.events.single.y), (0x800005, 'blink', 10.5, 3.25));
      final d = ServerMsg.decode(hex('a201066861737465640001480164ffd1ff0402054d6167696304526172654b321414'))! as AffixDefs;
      expect(d.defs.map((x) => (x.id, x.bit, x.name, x.desc, x.color)), [('hasted', 0, 'H', 'd', 0x9fe8ff)]);
      expect(d.ranks, ['Magic', 'Rare']);
      expect((d.expPct, d.dropPct, d.extraPct, d.hpPct), (75, 50, 20, 20));
      expect(d.toJson()['defs'], [
        {'id': 'hasted', 'bit': 0, 'name': 'H', 'desc': 'd', 'color': 0x9fe8ff},
      ]);
    });

    test('results goldens (protocol/results.rs `goldens`)', () {
      final s =
          ServerMsg.decode(
                hex(
                  '81017d0a01ac0203416e6e046d6167651f02dc0bc801280c030100020901a01f02ac0200020014000204626f6c74b0090a0661747461636bac0206013d000008416e67656c696e67',
                ),
              )!
              as RunStats;
      expect((s.victory, s.secs, s.bucketSecs), (true, 125, 10));
      final seat = s.seats.single;
      expect((seat.entityId, seat.name, seat.cls, seat.level, seat.flags), (300, 'Ann', 'mage', 31, RunSeat.linkDead));
      expect((seat.dealt, seat.taken, seat.healed, seat.kills, seat.assists), (1500, 200, 40, 12, 3));
      expect((seat.downs, seat.deaths, seat.revives, seat.bestCombo, seat.mvps, seat.exp), (1, 0, 2, 9, 1, 4000));
      expect(seat.buckets, [(300, 0, 2), (0, 20, 0)]);
      expect(seat.skills, [('bolt', 1200, 10), ('attack', 300, 6)]);
      expect(seat.toJson()['linkDead'], isTrue);
      expect(s.marks.map((m) => (m.secs, m.kind, m.seat, m.label)), [(61, 0, 0, 'Angeling')]);
      final r = ServerMsg.decode(hex('8200a01f8407fa01643201076a656c6c6f707907'))! as RunRewards;
      expect((r.seat, r.exp, r.zeny, r.clearZeny, r.modExp, r.modZeny), (0, 4000, 900, 250, 100, 50));
      expect(r.items, [('jellopy', 7)]);
    });

    test('emote, pose and flag frames (protocol.rs header layout)', () {
      expect(toHex(ClientMsg.emote(46)), '512e');
      expect(toHex(ClientMsg.pose(sitting: true)), '5201');
      expect(toHex(ClientMsg.flag(3, 1.5, 2.25)), '530360009000');
      final e =
          ServerMsg.decode(
                hex(
                  '378080c003'
                  '2e',
                ),
              )!
              as Emote;
      expect((e.entityId, e.emote), (0x700000, 46));
      final p = ServerMsg.decode(hex('380901'))! as Pose;
      expect((p.entityId, p.sitting), (9, true));
      final f =
          ServerMsg.decode(
                hex(
                  '390909'
                  '6000'
                  '9000',
                ),
              )!
              as FlagPing;
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
                  '210166054669656c64029003020000003f00000034c200002041000020420000a04106612e6f676702006000900000010902426f066e6f76696365c000000106010402031f03626f77',
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
      expect(p.weapon, 'bow');

      final lobby =
          (ServerMsg.decode(
                    hex(
                      '310105414232434416706f72696e675f69736c616e645f737572766976616c16506f72696e672049736c616e6420537572766976616c48486f6c64206f757420616761696e73742067726f77696e67207761766573206f6e20506f72696e672049736c616e6420756e74696c2074686520626f73732073686f77732075702e0a7072745f66696c6430380800010902426f066e6f76696365060100',
                    ),
                  )!
                  as LobbyState)
              .lobby;
      expect(
        (lobby.lobbyId, lobby.code, lobby.public, lobby.mission.id, lobby.mission.maxPlayers),
        (1, 'AB2CD', false, 'poring_island_survival', 8),
      );
      final m = lobby.members.single;
      expect((m.entityId, m.name, m.leader, m.online, m.away, m.level, m.offlineSecs), (9, 'Bo', true, true, false, 1, 0));

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
      final c = ServerMsg.decode(hex('2b01ac0206878080040287808004c80115ac020002'))! as Combat;
      expect(
        (c.swings.single.entityId, c.swings.single.facing, c.swings.single.target),
        (300, 6, monsterEntityBase + 7),
      );
      final kill = c.hits[0], miss = c.hits[1];
      expect(
        (kill.target, kill.amount, kill.isCritical, kill.isKill, kill.damageType),
        (monsterEntityBase + 7, 200, true, true, 1),
      );
      expect((miss.target, miss.isMiss, miss.amount), (300, true, 0));
      final pr = ServerMsg.decode(hex('2c1fd8a705e8f505e102'))! as Progress;
      expect((pr.level, pr.exp, pr.expToNext, pr.gained), (31, 87000, 97000, 353));
      final sk = ServerMsg.decode(hex('2d0c030104626f6c74040204626f6c7400'))! as Skills;
      expect((sk.level, sk.points), (12, 3));
      expect(sk.levels, {'bolt': 4});
      expect(sk.hotbar, ['bolt', '']);
      final cast =
          ServerMsg.decode(hex('2eac0204626f6c74a002d0009c04e80701090102a002d0004000024000f02eac0203'))! as Cast;
      expect(
        (cast.caster, cast.skill, cast.x, cast.y, cast.castMs, cast.cooldownMs),
        (300, 'bolt', 10.5, 3.25, 540, 1000),
      );
      expect(cast.level, 3);
      final t = cast.areas.single;
      expect(
        (t.id, t.shape, t.look, t.x, t.y, t.dir, t.a, t.b, t.spanMs, t.follow),
        (9, 1, 2, 10.5, 3.25, 64, 8.0, 1.0, 6000, 300),
      );
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
        hex(
          '030400640000000c000000010009000000600080000201041a05018002000510f0048001fe030080'
          '9902ffff00007f0103',
        ),
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

  group('click to walk (WalkGrid.findPath)', () {
    // A wall with one gap at the bottom between the player (west) and an
    // NPC (east, standing on a blocked cell).
    final room = WalkGrid.ascii([
      '############',
      '#....#.....#',
      '#....#..#..#',
      '#....#.....#',
      '#..........#',
      '############',
    ]);

    test('walks around a wall into reach and every leg is clear', () {
      const tx = 8.5, ty = 3.5, reach = 1.5;
      final path = room.findPath(2.5, 3.5, tx, ty, reach)!;
      expect(path, isNotEmpty);
      final (ex, ey) = path.last;
      expect(math.sqrt((ex - tx) * (ex - tx) + (ey - ty) * (ey - ty)), lessThanOrEqualTo(reach));
      var (ax, ay) = (2.5, 3.5);
      for (final (x, y) in path) {
        expect(room.clearLine(ax, ay, x, y), isTrue, reason: '($ax,$ay) -> ($x,$y)');
        (ax, ay) = (x, y);
      }
      expect(path.any((p) => p.$2 < 2), isTrue, reason: 'through the gap in row y = 1');
      expect(path.length, lessThan(6), reason: 'straightened');
    });

    test('already in reach, walled in, or starting in a wall', () {
      expect(room.findPath(7.5, 3.5, 8.5, 3.5, 1.5), isEmpty);
      final shut = WalkGrid.ascii(['#####', '#.#.#', '#####']);
      expect(shut.findPath(1.5, 1.5, 3.5, 1.5, 0.5), isNull);
      expect(room.findPath(0.5, 0.5, 8.5, 3.5, 1.5), isNull);
    });

    test('following the path with the server movement rules gets there', () {
      const tx = 8.5, ty = 3.5, reach = 1.5;
      final path = room.findPath(2.5, 3.5, tx, ty, reach)!;
      const rules = MoveRules();
      var (x, y) = (2.5, 3.5);
      var i = 0;
      for (var tick = 0; tick < 200 && i < path.length; tick++) {
        final (wx, wy) = path[i];
        if ((wx - x).abs() < 0.2 && (wy - y).abs() < 0.2) {
          i++;
          continue;
        }
        (x, y) = rules.apply(room, x, y, wx - x, wy - y);
      }
      expect(i, path.length, reason: 'reached every point');
      // Within 0.2 per axis of the last point, which is in reach.
      expect((x - tx) * (x - tx) + (y - ty) * (y - ty), lessThanOrEqualTo((reach + 0.3) * (reach + 0.3)));
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

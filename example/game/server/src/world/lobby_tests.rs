use super::*;
use crate::world::hub::test_support::map;
use crate::world::hub::HubWorld;
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::stage::StageConfig;

const MISSION: &str = "poring_island_survival";

fn stages(max_stages: usize) -> StageDirectory {
    let field = map(
        "prt_fild08",
        LocationKind::Field,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
        &["#####", "#...#", "#...#", "#####"],
        vec![],
    );
    StageDirectory::new(vec![HubWorld::new(vec![field], "prt_fild08").unwrap()], StageConfig { max_stages, ..Default::default() })
}

fn lobbies(max_stages: usize) -> LobbyDirectory {
    LobbyDirectory::new(stages(max_stages))
}

fn profile(user_id: u32) -> PlayerProfile {
    PlayerProfile {
        user_id,
        character_id: 100 + user_id,
        name: format!("p{user_id}"),
        archetype: "novice".into(),
        look: crate::world::protocol::Look::default(),
        level: 1,
        exp: 0,
        weapon: String::new(),
        loadout: Default::default(),
        bag: Default::default(),
    }
}

fn sid(user_id: u32) -> Uuid {
    Uuid::from_u128(user_id as u128)
}

struct Client {
    user: u32,
    tx: mpsc::Sender<LobbyEvent>,
    rx: mpsc::Receiver<LobbyEvent>,
    /// The last `MODIFIERS` state seen.
    mods: Option<ServerMsg>,
}

impl Client {
    fn new(user: u32) -> Self {
        let (tx, rx) = mpsc::channel(LOBBY_CHANNEL_CAPACITY);
        Self { user, tx, rx, mods: None }
    }

    /// Drains events; returns the last party state and anything else
    /// (the modifier table, briefs and state aside: [`Client::mods`]).
    fn drain(&mut self) -> (Option<LobbyInfo>, Vec<LobbyEvent>) {
        let mut state = None;
        let mut other = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            match e {
                LobbyEvent::Msg(ServerMsg::LobbyState { lobby }) => state = Some(lobby),
                LobbyEvent::Msg(ServerMsg::ModifierDefs { .. } | ServerMsg::MissionInfo { .. }) => {}
                LobbyEvent::Msg(m @ ServerMsg::Modifiers { .. }) => self.mods = Some(m),
                e => other.push(e),
            }
        }
        (state, other)
    }

    fn select(&self, dir: &LobbyDirectory, public: bool) -> Result<u32, LobbyError> {
        dir.select(profile(self.user), sid(self.user), MISSION, public, self.tx.clone())
    }

    /// Back in town on its socket (after a mission, a reconnect).
    fn attach(&self, dir: &LobbyDirectory) -> bool {
        dir.attach(&profile(self.user), sid(self.user), self.tx.clone(), false)
    }
}

fn launched(events: &[LobbyEvent]) -> Option<u32> {
    events.iter().find_map(|e| match e {
        LobbyEvent::Launch(t) => Some(t.stage_id),
        _ => None,
    })
}

#[tokio::test]
async fn public_parties_fill_to_eight_then_open_another() {
    let dir = lobbies(4);
    let mut clients: Vec<Client> = (1..=9).map(Client::new).collect();
    let ids: Vec<u32> = clients.iter().map(|c| c.select(&dir, true).unwrap()).collect();
    assert_eq!(&ids[..8], &[ids[0]; 8], "the first eight share a party");
    assert_ne!(ids[8], ids[0], "the ninth opens a new one");
    let state = clients[0].drain().0.unwrap();
    assert_eq!(state.members.len(), 8);
    assert!(state.members[0].leader && !state.members[1].leader);
    assert!(state.members.iter().all(|m| m.online && !m.away));
    assert_eq!(state.code.len(), CODE_LEN);
    assert_eq!(state.mission.max_players, PARTY_MAX);
}

#[tokio::test]
async fn private_party_only_by_code() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let b = Client::new(2);
    let c = Client::new(3);
    let private = a.select(&dir, false).unwrap();
    let public = b.select(&dir, true).unwrap();
    assert_ne!(private, public, "public matchmaking skips private parties");
    let code = a.drain().0.unwrap().code;
    assert_eq!(dir.join_code(profile(3), sid(3), &code.to_lowercase(), c.tx.clone()).unwrap(), private, "codes are case-insensitive");
    assert_eq!(dir.join_code(profile(3), sid(3), &code, c.tx.clone()).unwrap(), private, "one's own code is fine");
    let bcode = dir.state.lock().unwrap().lobbies[&public].code.clone();
    assert_eq!(dir.join_code(profile(3), sid(3), &bcode, c.tx.clone()).unwrap_err(), LobbyError::AlreadyInLobby);
    assert_eq!(dir.join_code(profile(4), sid(4), "ZZZZZ", c.tx.clone()).unwrap_err(), LobbyError::NoSuchLobby);
    assert_eq!(a.drain().0.unwrap().members.len(), 2);
}

#[tokio::test]
async fn the_leader_starts_at_once_and_the_party_survives_the_mission() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mut b = Client::new(2);
    let id = a.select(&dir, true).unwrap();
    b.select(&dir, true).unwrap();
    assert_eq!(dir.start(2, sid(2)).unwrap_err(), LobbyError::NotLeader);
    // No ready check: the leader goes the moment they like.
    let stage = dir.start(1, sid(1)).unwrap();
    for c in [&mut a, &mut b] {
        let (state, events) = c.drain();
        assert_eq!(launched(&events), Some(stage), "{events:?}");
        assert!(state.unwrap().members.iter().all(|m| m.away), "the party shows who is on the mission");
    }
    assert_eq!(dir.open_lobbies(), 1, "the party stays");
    assert_eq!(dir.lobby_of(1), Some(id));
    assert_eq!(dir.stages().active(), 1);

    // The leader is on the mission: starts come from town.
    assert_eq!(dir.start(1, sid(1)).unwrap_err(), LobbyError::OnMission);

    // Back in town: they are in the same party, ready to go again.
    assert!(a.attach(&dir) && b.attach(&dir));
    let state = b.drain().0.unwrap();
    assert_eq!(state.lobby_id, id);
    assert!(state.members.iter().all(|m| m.online && !m.away));
    let again = dir.start(1, sid(1)).unwrap();
    assert_ne!(again, stage);
    assert_eq!(launched(&b.drain().1), Some(again));
}

#[tokio::test]
async fn only_members_in_town_go_along() {
    let dir = lobbies(4);
    let (mut a, mut b, mut c) = (Client::new(1), Client::new(2), Client::new(3));
    for x in [&a, &b, &c] {
        x.select(&dir, true).unwrap();
    }
    dir.detach(3, sid(3));
    let state = a.drain().0.unwrap();
    assert!(!state.members[2].online && state.members.len() == 3, "offline, still in the party");
    let first = dir.start(1, sid(1)).unwrap();
    assert_eq!(launched(&b.drain().1), Some(first));
    assert_eq!(launched(&c.drain().1), None, "offline: stays behind");
    // b comes back from the stage first; the leader is still away: b is
    // not the leader, and c (back online) waits for the leader.
    b.attach(&dir);
    c.attach(&dir);
    assert_eq!(dir.start(2, sid(2)).unwrap_err(), LobbyError::NotLeader);
    a.attach(&dir);
    a.drain();
    let second = dir.start(1, sid(1)).unwrap();
    assert_eq!(launched(&c.drain().1), Some(second));
    // A member on a mission is left out too.
    dir.attach(&profile(2), sid(2), b.tx.clone(), true);
    a.attach(&dir);
    c.attach(&dir);
    b.drain();
    let third = dir.start(1, sid(1)).unwrap();
    assert_eq!(launched(&b.drain().1), None, "b is still on a mission");
    assert_eq!(launched(&c.drain().1), Some(third));
}

#[tokio::test]
async fn switching_characters_and_signing_out_keep_the_membership() {
    let dir = lobbies(4);
    let (mut a, b) = (Client::new(1), Client::new(2));
    let id = a.select(&dir, false).unwrap();
    let code = a.drain().0.unwrap().code;
    dir.join_code(profile(2), sid(2), &code, b.tx.clone()).unwrap();
    // b signs out, then plays another character of the account.
    dir.detach(2, sid(2));
    let state = a.drain().0.unwrap();
    assert!(!state.members[1].online);
    assert_eq!(state.members[1].name, "p2", "their last character while away");
    let alt = PlayerProfile { character_id: 777, name: "Alt".into(), archetype: "mage".into(), level: 9, ..profile(2) };
    let fresh = Uuid::from_u128(99);
    assert!(dir.attach(&alt, fresh, b.tx.clone(), false));
    let m = a.drain().0.unwrap().members[1].clone();
    assert_eq!((m.entity_id, m.name.as_str(), m.archetype.as_str(), m.level, m.online), (777, "Alt", "mage", 9, true));
    assert_eq!(dir.lobby_of(2), Some(id));
    // The old socket's late cleanup leaves the new one alone.
    dir.detach(2, sid(2));
    assert!(a.drain().0.is_none(), "nothing changed");
    assert!(!dir.leave(2, sid(2), "left"), "and it cannot act");
    // Kicked by entity id of the current character.
    dir.kick(1, sid(1), 777).unwrap();
    assert_eq!(dir.lobby_of(2), None);
}

#[tokio::test]
async fn solo_leader_can_start_and_stage_full_keeps_the_party() {
    let dir = lobbies(1);
    let a = Client::new(1);
    let b = Client::new(2);
    a.select(&dir, false).unwrap();
    b.select(&dir, false).unwrap();
    dir.start(1, sid(1)).unwrap();
    assert_eq!(dir.start(2, sid(2)).unwrap_err(), LobbyError::StageFull);
    assert!(dir.lobby_of(2).is_some(), "the party survives a failed start");
}

#[tokio::test]
async fn leader_leaving_promotes_next_and_kick_works() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mut b = Client::new(2);
    let mut c = Client::new(3);
    for x in [&a, &b, &c] {
        x.select(&dir, true).unwrap();
    }
    assert!(dir.leave(1, sid(1), "left"));
    let (_, events) = a.drain();
    assert!(matches!(&events[..], [LobbyEvent::Msg(ServerMsg::LobbyLeft { reason })] if reason == "left"));
    let state = b.drain().0.unwrap();
    assert_eq!(state.members.len(), 2);
    assert!(state.members[0].leader && state.members[0].entity_id == 102, "p2 leads now");

    assert_eq!(dir.kick(3, sid(3), 102).unwrap_err(), LobbyError::NotLeader);
    assert_eq!(dir.kick(2, sid(2), 102).unwrap_err(), LobbyError::NoSuchMember, "cannot kick yourself");
    // Offline members can be kicked too.
    dir.detach(3, sid(3));
    dir.kick(2, sid(2), 103).unwrap();
    assert!(c.drain().1.is_empty(), "an offline member gets nothing");
    assert!(dir.leave(2, sid(2), "left"));
    assert_eq!(dir.open_lobbies(), 0, "an empty party closes");
}

#[tokio::test]
async fn an_absent_leader_hands_over_and_long_absences_drop_out() {
    let dir = lobbies(4);
    let (mut a, b, c) = (Client::new(1), Client::new(2), Client::new(3));
    for x in [&a, &b, &c] {
        x.select(&dir, true).unwrap();
    }
    dir.detach(1, sid(1));
    dir.detach(2, sid(2));
    let ago = |d: Duration| Utc::now() - chrono::Duration::from_std(d).unwrap();
    // A short absence (a reload) keeps the lead.
    dir.reap(Utc::now());
    assert_eq!(dir.start(3, sid(3)).unwrap_err(), LobbyError::NotLeader);
    // Past the grace the oldest *online* member leads; the old leader stays.
    dir.state.lock().unwrap().lobbies.values_mut().next().unwrap().members[0].last_seen = ago(LEADER_GRACE);
    assert!(dir.start(3, sid(3)).is_ok(), "the lead passed on the caller's action");
    let names = |dir: &LobbyDirectory| {
        let st = dir.state.lock().unwrap();
        st.lobbies.values().next().map(|l| l.members.iter().map(|m| m.user_id).collect::<Vec<_>>())
    };
    assert_eq!(names(&dir), Some(vec![3, 1, 2]));
    // Two weeks away: out of the party; the reaper does it unasked.
    for m in &mut dir.state.lock().unwrap().lobbies.values_mut().next().unwrap().members[1..] {
        m.last_seen = ago(MEMBER_TTL);
    }
    dir.reap(Utc::now());
    assert_eq!(names(&dir), Some(vec![3]));
    assert_eq!(dir.lobby_of(1), None);
    // The last one gone too: the party closes.
    dir.detach(3, sid(3));
    dir.state.lock().unwrap().lobbies.values_mut().next().unwrap().members[0].last_seen = ago(MEMBER_TTL);
    dir.reap(Utc::now());
    assert_eq!(dir.open_lobbies(), 0);
    a.drain();
}

#[tokio::test]
async fn the_leader_picks_another_mission_members_cannot() {
    let dir = lobbies(4);
    let (a, b) = (Client::new(1), Client::new(2));
    let id = a.select(&dir, true).unwrap();
    b.select(&dir, true).unwrap();
    // Same mission: nothing changes, the party stays theirs.
    assert_eq!(a.select(&dir, false).unwrap(), id);
    assert_eq!(b.select(&dir, true).unwrap(), id);
    assert!(matches!(dir.select(profile(1), sid(1), "nope", true, a.tx.clone()), Err(LobbyError::UnknownMission(_))));
    // Set for another mission (this server has one only): a member is told
    // who picks; the leader's pick switches the party.
    static OTHER: MissionInfo = MissionInfo {
        id: "other",
        name: "Other",
        map: "prt_fild08",
        max_players: PARTY_MAX,
        fog: None,
        ..MissionInfo::TEMPLATE
    };
    dir.state.lock().unwrap().lobbies.get_mut(&id).unwrap().mission = &OTHER;
    assert_eq!(b.select(&dir, true).unwrap_err(), LobbyError::PartyMission("Other".into()));
    assert_eq!(a.select(&dir, true).unwrap(), id);
    assert_eq!(dir.state.lock().unwrap().lobbies[&id].mission.id, MISSION);
    // The leader's quick start does the same.
    dir.state.lock().unwrap().lobbies.get_mut(&id).unwrap().mission = &OTHER;
    assert!(dir.quick_start(profile(1), sid(1), MISSION, 0, 0, a.tx.clone()).is_ok());
    assert_eq!(dir.state.lock().unwrap().lobbies[&id].mission.id, MISSION);
}

#[tokio::test]
async fn stale_session_cannot_act() {
    let dir = lobbies(4);
    let mut old = Client::new(1);
    old.select(&dir, true).unwrap();
    old.drain();
    // Same account from a new session (a reconnect): the membership moves.
    let fresh = Client::new(1);
    dir.attach(&profile(1), Uuid::from_u128(99), fresh.tx.clone(), false);
    assert!(!dir.leave(1, sid(1), "left"), "the old session's late leave is ignored");
    assert_eq!(dir.start(1, sid(1)).unwrap_err(), LobbyError::NotInLobby);
    assert!(dir.lobby_of(1).is_some());
    assert!(dir.start(1, Uuid::from_u128(99)).is_ok());
}

fn invite(dir: &LobbyDirectory, from: &Client, target: u32) -> Result<String, LobbyError> {
    dir.invite(profile(from.user), sid(from.user), target, from.tx.clone())
}

fn answer(dir: &LobbyDirectory, to: &Client, from: u32, accept: bool) -> Result<(), LobbyError> {
    dir.answer(profile(to.user), sid(to.user), from, accept, to.tx.clone())
}

fn answers(events: &[LobbyEvent]) -> Vec<(u32, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            LobbyEvent::Msg(ServerMsg::PartyAnswer { entity_id, accepted, .. }) => Some((*entity_id, *accepted)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn invitation_without_a_party_opens_a_private_one_for_both() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mut b = Client::new(2);
    assert_eq!(invite(&dir, &a, 102).unwrap(), "Poring Island Survival");
    assert_eq!(dir.open_lobbies(), 0, "nothing opens before the answer");
    answer(&dir, &b, 101, true).unwrap();
    let (state, events) = a.drain();
    let state = state.unwrap();
    assert!(!state.public);
    assert_eq!(state.members.iter().map(|m| (m.entity_id, m.leader, m.online)).collect::<Vec<_>>(), vec![(101, true, true), (102, false, true)]);
    assert_eq!(answers(&events), vec![(102, true)]);
    assert_eq!(b.drain().0.unwrap().members.len(), 2);
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired, "used up");
    assert!(dir.start(1, sid(1)).is_ok(), "the inviter leads on their socket");
}

#[tokio::test]
async fn leader_invites_into_the_party_and_decline_tells_the_inviter() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let b = Client::new(2);
    let c = Client::new(3);
    let id = a.select(&dir, true).unwrap();
    invite(&dir, &a, 102).unwrap();
    invite(&dir, &a, 103).unwrap();
    answer(&dir, &b, 101, false).unwrap();
    assert_eq!(answers(&a.drain().1), vec![(102, false)]);
    assert_eq!(dir.lobby_of(2), None);
    answer(&dir, &c, 101, true).unwrap();
    assert_eq!(dir.lobby_of(3), Some(id), "joins the leader's party");
    assert_eq!(a.drain().0.unwrap().members.len(), 2);
    assert_eq!(answer(&dir, &c, 999, true).unwrap_err(), LobbyError::InviteExpired, "nobody invited from there");
}

#[tokio::test]
async fn invitation_rules() {
    let dir = lobbies(4);
    let clients: Vec<Client> = (1..=8).map(Client::new).collect();
    let a = &clients[0];
    assert_eq!(invite(&dir, a, 101).unwrap_err(), LobbyError::NoSuchPlayer, "not oneself");
    a.select(&dir, true).unwrap();
    clients[1].select(&dir, true).unwrap();
    assert_eq!(invite(&dir, &clients[1], 103).unwrap_err(), LobbyError::NotLeader);
    assert_eq!(invite(&dir, &clients[2], 102).unwrap_err(), LobbyError::InParty, "the target is in a party");
    assert_eq!(invite(&dir, a, 102).unwrap_err(), LobbyError::InParty, "already in yours");
    // A full party takes no invitations.
    for c in &clients[2..] {
        c.select(&dir, true).unwrap();
    }
    assert_eq!(invite(&dir, a, 109).unwrap_err(), LobbyError::LobbyFull);
}

#[tokio::test]
async fn invitations_lapse_when_the_inviter_moves_on() {
    let dir = lobbies(4);
    let a = Client::new(1);
    let b = Client::new(2);
    let c = Client::new(3);
    // The invitee joined another party meanwhile.
    invite(&dir, &a, 102).unwrap();
    c.select(&dir, false).unwrap();
    let code = dir.state.lock().unwrap().lobbies.values().next().unwrap().code.clone();
    dir.join_code(profile(2), sid(2), &code, b.tx.clone()).unwrap();
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::AlreadyInLobby);
    assert!(dir.leave(2, sid(2), "left"));

    // The inviter (no party) joined one since: the invitation is void.
    invite(&dir, &a, 102).unwrap();
    dir.join_code(profile(1), sid(1), &code, a.tx.clone()).unwrap();
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired);
    assert!(dir.leave(1, sid(1), "left"));

    // The leader left the party they invited into.
    invite(&dir, &c, 102).unwrap();
    assert!(dir.leave(3, sid(3), "left"));
    assert_eq!(answer(&dir, &b, 103, true).unwrap_err(), LobbyError::InviteExpired);

    // Forgotten (the inviter left town), signed out, and timed out.
    invite(&dir, &a, 102).unwrap();
    dir.forget_invites(1);
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired);
    invite(&dir, &a, 102).unwrap();
    dir.detach(1, sid(1));
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired);
    invite(&dir, &a, 102).unwrap();
    dir.state.lock().unwrap().invites[0].at -= INVITE_TTL;
    assert_eq!(answer(&dir, &b, 101, false).unwrap_err(), LobbyError::InviteExpired);
    assert_eq!(dir.open_lobbies(), 0);
}

#[tokio::test]
async fn launching_drops_the_party_s_invitations() {
    let dir = lobbies(4);
    let a = Client::new(1);
    let b = Client::new(2);
    a.select(&dir, false).unwrap();
    invite(&dir, &a, 102).unwrap();
    dir.start(1, sid(1)).unwrap();
    assert_eq!(answer(&dir, &b, 101, true).unwrap_err(), LobbyError::InviteExpired, "the inviter is on a mission");
    assert_eq!(dir.lobby_of(2), None);
}

#[tokio::test]
async fn unknown_and_unavailable_missions() {
    let dir = lobbies(4);
    let a = Client::new(1);
    assert_eq!(dir.select(profile(1), sid(1), "nope", true, a.tx.clone()).unwrap_err(), LobbyError::UnknownMission("nope".into()));
    let empty = LobbyDirectory::new(StageDirectory::new(vec![], StageConfig::default()));
    assert!(matches!(empty.select(profile(1), sid(1), MISSION, true, a.tx.clone()), Err(LobbyError::MissionUnavailable(_))));
}

#[tokio::test]
async fn leader_sets_modifiers_members_see_them_and_the_launch_carries_them() {
    use crate::world::modifiers::{ModFlags, MAX_TIER};
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mut b = Client::new(2);
    a.select(&dir, true).unwrap();
    b.select(&dir, true).unwrap();
    let mask = ModFlags::SLOW.0 | ModFlags::DARKNESS.0;
    assert_eq!(dir.set_modifiers(2, sid(2), 3, mask).unwrap_err(), LobbyError::NotLeader);
    assert!(matches!(dir.set_modifiers(1, sid(1), MAX_TIER + 1, 0), Err(LobbyError::BadModifier(_))));
    assert!(matches!(dir.set_modifiers(1, sid(1), 0, ModFlags::VOLATILE.0), Err(LobbyError::BadModifier(_))), "locked");
    assert!(matches!(dir.set_modifiers(1, sid(1), 0, 1 << 31), Err(LobbyError::BadModifier(_))), "unknown bit");
    assert!(matches!(dir.set_modifiers(1, sid(1), 0, ModFlags::ONE_HIT.0), Err(LobbyError::BadModifier(_))), "level 1 leader");
    a.drain();
    b.drain();
    dir.set_modifiers(1, sid(1), 3, mask).unwrap();
    for c in [&mut a, &mut b] {
        c.drain();
        assert!(
            matches!(c.mods, Some(ServerMsg::Modifiers { scope: 0, tier: 3, mask: m, risk, .. }) if m == mask && risk == 12 + 6 + 8),
            "{:?}",
            c.mods
        );
    }
    // A late joiner gets the table and the current choice.
    let mut c = Client::new(3);
    c.select(&dir, true).unwrap();
    let mut table = false;
    while let Ok(e) = c.rx.try_recv() {
        table |= matches!(e, LobbyEvent::Msg(ServerMsg::ModifierDefs { .. }));
        if let LobbyEvent::Msg(m @ ServerMsg::Modifiers { .. }) = e {
            c.mods = Some(m);
        }
    }
    assert!(table && matches!(c.mods, Some(ServerMsg::Modifiers { tier: 3, .. })));
    assert!(dir.leave(3, sid(3), "left"));
    let stage = dir.start(1, sid(1)).unwrap();
    assert_eq!(dir.stages().modifiers(stage), Some(Modifiers { tier: 3, flags: ModFlags(mask) }));
}

#[tokio::test]
async fn quick_start_alone_launches_at_once_with_the_modifiers() {
    use crate::world::modifiers::ModFlags;
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mask = ModFlags::SLOW.0;
    assert!(
        matches!(dir.quick_start(profile(1), sid(1), MISSION, 0, ModFlags::ONE_HIT.0, a.tx.clone()), Err(LobbyError::BadModifier(_))),
        "the curses' levels hold for a party of one too"
    );
    assert!(matches!(dir.quick_start(profile(1), sid(1), "nope", 0, 0, a.tx.clone()), Err(LobbyError::UnknownMission(_))));
    let stage = dir.quick_start(profile(1), sid(1), MISSION, 2, mask, a.tx.clone()).unwrap();
    let (state, events) = a.drain();
    assert!(state.is_none(), "no party is ever listed");
    assert!(matches!(&events[..], [LobbyEvent::Launch(t)] if t.stage_id == stage && t.mission == MISSION), "{events:?}");
    assert_eq!(dir.open_lobbies(), 0);
    assert_eq!(dir.lobby_of(1), None);
    assert_eq!(dir.stages().modifiers(stage), Some(Modifiers { tier: 2, flags: ModFlags(mask) }));
}

#[tokio::test]
async fn quick_start_in_a_party_starts_it_with_the_window_s_modifiers() {
    let dir = lobbies(4);
    let mut a = Client::new(1);
    let mut b = Client::new(2);
    a.select(&dir, true).unwrap();
    b.select(&dir, true).unwrap();
    assert_eq!(dir.quick_start(profile(2), sid(2), MISSION, 0, 0, b.tx.clone()).unwrap_err(), LobbyError::NotLeader);
    assert!(matches!(dir.quick_start(profile(1), sid(1), "other", 0, 0, a.tx.clone()), Err(LobbyError::UnknownMission(_))));
    let stage = dir.quick_start(profile(1), sid(1), MISSION, 1, 0, a.tx.clone()).unwrap();
    for c in [&mut a, &mut b] {
        assert_eq!(launched(&c.drain().1), Some(stage));
    }
    assert_eq!(dir.stages().modifiers(stage).map(|m| m.tier), Some(1));
    assert_eq!(dir.open_lobbies(), 1, "the party stays");
}

#[tokio::test]
async fn a_curse_s_level_holds_the_start_for_everyone_going() {
    use crate::world::modifiers::ModFlags;
    let dir = lobbies(4);
    let strong = |u: u32| PlayerProfile { level: 30, ..profile(u) };
    let (a, b) = (Client::new(1), Client::new(2));
    dir.select(strong(1), sid(1), MISSION, true, a.tx.clone()).unwrap();
    dir.select(profile(2), sid(2), MISSION, true, b.tx.clone()).unwrap();
    dir.set_modifiers(1, sid(1), 0, ModFlags::ONE_HIT.0).unwrap();
    assert!(matches!(dir.start(1, sid(1)), Err(LobbyError::BadModifier(m)) if m.starts_with("p2")));
    // p2 offline: they stay behind, the start goes.
    dir.detach(2, sid(2));
    assert!(dir.start(1, sid(1)).is_ok());
}

/// The database keeps parties across restarts: members, seats, leader,
/// code, mission and modifiers come back, everyone offline.
#[tokio::test]
async fn parties_survive_a_restart() {
    use crate::db::UserRepo;
    use crate::world::modifiers::ModFlags;
    let pool = crate::db::testing::pool().await;
    let mut users = Vec::new();
    for name in ["ann", "bob", "cid"] {
        users.push(UserRepo::create(&pool, name, "hash").await.unwrap().id as u32);
    }
    let dir = LobbyDirectory::persistent(stages(4), pool.clone()).await.unwrap();
    let pf = |i: usize| PlayerProfile { user_id: users[i], character_id: 100 + i as u32, name: format!("c{i}"), level: 10 + i as u32, ..profile(users[i]) };
    let sid_of = |i: usize| Uuid::from_u128(users[i] as u128);
    let clients: Vec<Client> = users.iter().map(|&u| Client::new(u)).collect();
    let id = dir.select(pf(0), sid_of(0), MISSION, false, clients[0].tx.clone()).unwrap();
    let code = dir.state.lock().unwrap().lobbies[&id].code.clone();
    dir.join_code(pf(1), sid_of(1), &code, clients[1].tx.clone()).unwrap();
    dir.join_code(pf(2), sid_of(2), &code, clients[2].tx.clone()).unwrap();
    dir.set_modifiers(users[0], sid_of(0), 2, ModFlags::SLOW.0).unwrap();
    dir.leave(users[2], sid_of(2), "left");
    dir.detach(users[1], sid_of(1));
    // A mission and back: still the party.
    dir.start(users[0], sid_of(0)).unwrap();
    dir.flush().await;

    let again = LobbyDirectory::persistent(stages(4), pool.clone()).await.unwrap();
    assert_eq!(again.lobby_of(users[0]), Some(id));
    assert_eq!(again.lobby_of(users[1]), Some(id));
    assert_eq!(again.lobby_of(users[2]), None, "who left stays out");
    {
        let st = again.state.lock().unwrap();
        let l = &st.lobbies[&id];
        assert_eq!((l.code.as_str(), l.mission.id, l.public), (code.as_str(), MISSION, false));
        assert_eq!(l.mods, Modifiers { tier: 2, flags: ModFlags::SLOW });
        assert_eq!(l.members.iter().map(|m| (m.user_id, m.who.name.as_str(), m.who.level)).collect::<Vec<_>>(), vec![(users[0], "c0", 10), (users[1], "c1", 11)]);
        assert!(l.members.iter().all(|m| !m.online() && !m.away), "everyone offline after a restart");
        assert_eq!(st.by_code.get(&code), Some(&id));
    }
    // Signing in again brings the member back online, in the same seat.
    let mut c = Client::new(users[1]);
    assert!(again.attach(&pf(1), sid_of(1), c.tx.clone(), false));
    let state = c.drain().0.unwrap();
    assert!(state.members[0].leader && !state.members[0].online && state.members[1].online);
    // New parties do not reuse the loaded ids.
    let fresh = Client::new(users[2]);
    let other = again.select(pf(2), sid_of(2), MISSION, true, fresh.tx.clone()).unwrap();
    assert!(other > id);
    // Leaving everyone empties and deletes it in the database too.
    again.attach(&pf(0), sid_of(0), clients[0].tx.clone(), false);
    again.leave(users[0], sid_of(0), "left");
    again.leave(users[1], sid_of(1), "left");
    again.flush().await;
    let rows = PartyRepo::load(&pool).await.unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), vec![other]);
}

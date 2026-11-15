//! The Kafra's party list and late joins (`public_parties`, `join_public`,
//! `set_public`, `StageDirectory::late_ticket`).

use super::*;
use crate::world::directory::HubCommand;
use crate::world::hub::test_support::map;
use crate::world::hub::{HubEvent, HubWorld};
use crate::world::map::{CameraConstraint, LocationKind};
use crate::world::protocol::{party_state, PartyListing};
use crate::world::stage::{StageConfig, StageError};
use crate::world::OUTBOX_CAPACITY;
use std::assert_matches;

const MISSION: &str = "poring_island_survival";

fn lobbies() -> LobbyDirectory {
    let row = format!("#{}#", ".".repeat(38));
    let wall = "#".repeat(40);
    let rows: Vec<&str> = vec![&wall, &row, &row, &row, &row, &row, &row, &wall];
    let field = map(
        "prt_fild08",
        LocationKind::Field,
        CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
        &rows,
        vec![],
    );
    let stages = StageDirectory::new(vec![HubWorld::new(vec![field], "prt_fild08").unwrap()], StageConfig { max_stages: 4, ..Default::default() });
    LobbyDirectory::new(stages)
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
        homunculus: None,
    }
}

fn sid(user_id: u32) -> Uuid {
    Uuid::from_u128(user_id as u128)
}

struct Client {
    user: u32,
    tx: mpsc::Sender<LobbyEvent>,
    rx: mpsc::Receiver<LobbyEvent>,
}

impl Client {
    fn new(user: u32) -> Self {
        let (tx, rx) = mpsc::channel(LOBBY_CHANNEL_CAPACITY);
        Self { user, tx, rx }
    }

    fn select(&self, dir: &LobbyDirectory, public: bool) -> u32 {
        dir.select(profile(self.user), sid(self.user), MISSION, public, self.tx.clone()).unwrap()
    }

    fn join(&self, dir: &LobbyDirectory, lobby: u32) -> Result<u32, LobbyError> {
        dir.join_public(profile(self.user), sid(self.user), lobby, self.tx.clone())
    }

    /// The tickets handed to this client since the last call.
    fn tickets(&mut self) -> Vec<StageTicket> {
        let mut out = Vec::new();
        while let Ok(e) = self.rx.try_recv() {
            if let LobbyEvent::Launch(t) = e {
                out.push(t);
            }
        }
        out
    }
}

fn listed(dir: &LobbyDirectory, user: u32) -> Vec<PartyListing> {
    dir.public_parties(user)
}

/// Enters the stage of `ticket` as the socket task does; returns its
/// command channel and outbox.
fn enter(dir: &LobbyDirectory, ticket: &StageTicket, user: u32) -> (mpsc::Sender<HubCommand>, mpsc::Receiver<HubEvent>) {
    let (tx, rx) = mpsc::channel(OUTBOX_CAPACITY);
    let commands = dir.stages().enter(ticket.stage_id, profile(user), sid(user), tx, 0).unwrap();
    (commands, rx)
}

async fn whereabouts(commands: &mpsc::Sender<HubCommand>, user_id: u32) -> (f32, f32) {
    for _ in 0..50 {
        let (reply, rx) = tokio::sync::oneshot::channel();
        commands.send(HubCommand::Whereabouts { user_id, reply }).await.unwrap();
        if let Some(a) = rx.await.unwrap() {
            return (a.x, a.y);
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("user {user_id} never arrived");
}

#[tokio::test]
async fn the_list_shows_public_parties_with_someone_online_and_one_s_own() {
    let dir = lobbies();
    let (a, b, c) = (Client::new(1), Client::new(2), Client::new(3));
    let public = a.select(&dir, true);
    let private = b.select(&dir, false);
    assert_ne!(public, private);

    let seen = listed(&dir, 3);
    assert_eq!(seen.iter().map(|p| p.lobby_id).collect::<Vec<_>>(), vec![public], "private parties are not listed");
    let p = &seen[0];
    assert_eq!((p.mission.as_str(), p.state, p.elapsed_secs, p.mine), (MISSION, party_state::FORMING, 0, false));
    assert_eq!(p.members.len(), 1);
    assert_eq!((p.members[0].name.as_str(), p.members[0].online), ("p1", true));
    assert_eq!(p.capacity as usize, missions::find(MISSION).unwrap().max_players.min(PARTY_MAX) as usize);

    // One's own party is there, first, even when private.
    let own = listed(&dir, 2);
    assert_eq!((own[0].lobby_id, own[0].mine), (private, true));
    assert_eq!(own.len(), 2);

    // A party with nobody online drops off the list.
    dir.detach(1, sid(1));
    assert!(listed(&dir, 3).is_empty());
    let _ = c;
}

#[tokio::test]
async fn joining_a_forming_party_from_the_list_adds_the_player() {
    let dir = lobbies();
    let (a, b, mut c) = (Client::new(1), Client::new(2), Client::new(3));
    let id = a.select(&dir, true);
    assert_eq!(c.join(&dir, id).unwrap(), id);
    assert_eq!(dir.lobby_of(3), Some(id));
    assert!(c.tickets().is_empty(), "the party is in town: nobody goes anywhere");
    assert_eq!(listed(&dir, 1)[0].members.len(), 2);
    // Already in a party.
    let other = b.select(&dir, true);
    assert_eq!(other, id, "matchmaking found the same public party");
    assert_eq!(Client::new(4).join(&dir, 999).unwrap_err(), LobbyError::NoSuchLobby);
}

#[tokio::test]
async fn private_full_and_foreign_parties_cannot_be_joined_from_the_list() {
    let dir = lobbies();
    let (a, b, c) = (Client::new(1), Client::new(2), Client::new(3));
    let private = a.select(&dir, false);
    assert_eq!(c.join(&dir, private).unwrap_err(), LobbyError::NoSuchLobby, "private: by code only");
    let public = b.select(&dir, true);
    assert_eq!(a.join(&dir, public).unwrap_err(), LobbyError::AlreadyInLobby);
    let cap = missions::find(MISSION).unwrap().max_players.min(PARTY_MAX) as u32;
    for u in 10..(10 + cap - 1) {
        Client::new(u).join(&dir, public).unwrap();
    }
    assert_eq!(c.join(&dir, public).unwrap_err(), LobbyError::LobbyFull);
}

#[tokio::test]
async fn the_leader_lists_or_hides_the_party() {
    let dir = lobbies();
    let (a, b) = (Client::new(1), Client::new(2));
    let id = a.select(&dir, false);
    b.join(&dir, id).unwrap_err();
    assert_eq!(dir.set_public(2, sid(2), true).unwrap_err(), LobbyError::NotInLobby);
    dir.set_public(1, sid(1), true).unwrap();
    assert_eq!(listed(&dir, 2).len(), 1);
    b.join(&dir, id).unwrap();
    assert_eq!(dir.set_public(2, sid(2), false).unwrap_err(), LobbyError::NotLeader);
    dir.set_public(1, sid(1), false).unwrap();
    assert!(listed(&dir, 3).is_empty(), "hidden again");
    assert!(listed(&dir, 2)[0].mine, "members still see their own");
}

#[tokio::test]
async fn joining_a_party_on_a_mission_sends_the_player_into_the_run_beside_a_member() {
    let dir = lobbies();
    let (mut a, mut c) = (Client::new(1), Client::new(3));
    let id = a.select(&dir, true);
    let stage = dir.start(1, sid(1)).unwrap();
    let ticket = a.tickets().pop().expect("the leader goes");
    let (commands, _a_events) = enter(&dir, &ticket, 1);
    let (ax, ay) = whereabouts(&commands, 1).await;

    let row = listed(&dir, 3).into_iter().find(|p| p.lobby_id == id).unwrap();
    assert_eq!(row.state, party_state::ON_MISSION);

    // The leader walks off from the spawn; 3 joins from the list.
    commands.send(HubCommand::Input { user_id: 1, seq: 1, dx: 1.0, dy: 0.0, aim: [0, 0], slots: 0, prefs: Default::default(), uses: Default::default() }).await.unwrap();
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert_eq!(c.join(&dir, id).unwrap(), id);
    let late = c.tickets();
    assert_eq!(late.iter().map(|t| t.stage_id).collect::<Vec<_>>(), vec![stage], "a ticket into the running stage");
    let info = dir.state.lock().unwrap().lobbies[&id].info(Utc::now());
    assert!(info.members.iter().all(|m| m.away), "both are on the mission now");

    let (_, mut events) = enter(&dir, &late[0], 3);
    let (lx, ly) = whereabouts(&commands, 1).await;
    assert!(lx > ax, "the leader moved ({ax},{ay}) -> ({lx},{ly})");
    let (x, y) = whereabouts(&commands, 3).await;
    let d = (x - lx).hypot(y - ly);
    assert!(d <= 4.5, "the late joiner lands beside the leader: ({x},{y}) vs ({lx},{ly})");
    // Their client gets the stage like any arrival.
    let first = tokio::time::timeout(Duration::from_secs(2), events.recv()).await.unwrap().unwrap();
    assert_matches!(first, HubEvent::Control(ServerMsg::StageJoined { stage_id, .. }) if stage_id == stage, "{first:?}");
    assert_eq!(dir.stages().population(stage), Some(2));
}

#[tokio::test]
async fn a_member_back_in_town_early_rejoins_the_party_s_run() {
    let dir = lobbies();
    let (mut a, mut b) = (Client::new(1), Client::new(2));
    let id = a.select(&dir, true);
    b.join(&dir, id).unwrap();
    let stage = dir.start(1, sid(1)).unwrap();
    let (ta, tb) = (a.tickets().pop().unwrap(), b.tickets().pop().unwrap());
    let (_ca, _ea) = enter(&dir, &ta, 1);
    let (cb, _eb) = enter(&dir, &tb, 2);
    whereabouts(&cb, 2).await;
    // 2 walks home early and is in town again.
    assert!(dir.stages().leave(stage, 2, sid(2), true));
    dir.attach(&profile(2), sid(2), b.tx.clone(), false);
    // A member on the mission is not sent twice.
    assert_eq!(a.join(&dir, id).unwrap(), id);
    assert!(a.tickets().is_empty(), "1 is still on it");
    // 2 goes back.
    assert_eq!(b.join(&dir, id).unwrap(), id);
    assert_eq!(b.tickets().iter().map(|t| t.stage_id).collect::<Vec<_>>(), vec![stage]);
}

#[tokio::test]
async fn a_run_that_ended_takes_nobody_and_the_list_says_forming() {
    let dir = lobbies();
    let (mut a, mut c) = (Client::new(1), Client::new(3));
    let id = a.select(&dir, true);
    let stage = dir.start(1, sid(1)).unwrap();
    let t = a.tickets().pop().unwrap();
    let (_cmd, _ev) = enter(&dir, &t, 1);
    assert!(dir.stages().leave(stage, 1, sid(1), true));
    assert_eq!(dir.stages().age(stage), None, "the stage ended with its last player");
    assert_eq!(listed(&dir, 3)[0].state, party_state::FORMING);
    c.join(&dir, id).unwrap();
    assert!(c.tickets().is_empty());
}

#[tokio::test]
async fn a_late_ticket_respects_the_mission_s_player_limit() {
    static DUO: MissionInfo = MissionInfo { id: "duo", name: "Duo", map: "prt_fild08", max_players: 2, fog: None, ..MissionInfo::TEMPLATE };
    let dir = lobbies();
    let t = dir.stages().launch(&DUO, &[1, 2]).unwrap();
    assert_eq!(dir.stages().late_ticket(t.stage_id, 3).unwrap_err(), StageError::Full, "two coming already");
    dir.stages().abandon(t.stage_id, 2);
    let late = dir.stages().late_ticket(t.stage_id, 3).unwrap();
    assert_eq!(late.stage_id, t.stage_id);
    assert_eq!(dir.stages().late_ticket(999, 3).unwrap_err(), StageError::Gone(999));
}

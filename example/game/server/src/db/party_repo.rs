//! Account parties (`parties`, `party_members`; `migrations/0003_party.sql`).
//!
//! The live parties are in memory (`world::lobby`), which changes them
//! under a plain mutex on player actions. Every change is handed to a
//! [`PartyWriter`]: one task that writes the snapshots in the order they
//! were made (a member moving between parties is saved leaving one before
//! joining the other), so the tables follow the memory without the lobby
//! ever waiting on the database. Boot reads them back with
//! [`PartyRepo::load`].

use crate::db::Db;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::types::Json;
use tokio::sync::{mpsc, oneshot};
use tracing::error;

/// What the party window shows for a member: their current character, or
/// the last one while offline.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredCharacter {
    pub id: u32,
    pub name: String,
    pub archetype: String,
    pub level: u32,
}

/// The leader's difficulty tier and curse mask (`world::modifiers`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMods {
    #[serde(default)]
    pub tier: u8,
    #[serde(default)]
    pub mask: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MemberRow {
    pub user_id: u32,
    pub character: StoredCharacter,
    /// When they went offline; `None` while online.
    pub last_seen: Option<DateTime<Utc>>,
}

/// One party; `members[0]` leads (seat order).
#[derive(Clone, Debug, PartialEq)]
pub struct PartyRow {
    pub id: u32,
    pub code: String,
    pub mission: String,
    pub public: bool,
    pub mods: StoredMods,
    pub members: Vec<MemberRow>,
}

pub struct PartyRepo;

impl PartyRepo {
    /// Every party with its members in seat order (parties without members
    /// are left out; [`Self::delete`] them).
    pub async fn load(pool: &Db) -> Result<Vec<PartyRow>, sqlx::Error> {
        let parties: Vec<(i64, String, String, bool, Json<StoredMods>)> =
            sqlx::query_as("SELECT id, code, mission, public, modifiers FROM parties ORDER BY id").fetch_all(pool).await?;
        let members: Vec<(i64, i64, Json<StoredCharacter>, Option<DateTime<Utc>>)> = sqlx::query_as(
            "SELECT party_id, user_id, character, last_seen FROM party_members ORDER BY party_id, seat, joined_at",
        )
        .fetch_all(pool)
        .await?;
        let mut rows: Vec<PartyRow> = parties
            .into_iter()
            .map(|(id, code, mission, public, mods)| PartyRow { id: id as u32, code, mission, public, mods: mods.0, members: Vec::new() })
            .collect();
        for (party_id, user_id, character, last_seen) in members {
            if let Ok(i) = rows.binary_search_by_key(&(party_id as u32), |p| p.id) {
                rows[i].members.push(MemberRow { user_id: user_id as u32, character: character.0, last_seen });
            }
        }
        Ok(rows)
    }

    /// Writes `party` as it is now: the row, then its members (anyone no
    /// longer listed is dropped; anyone listed moves here from elsewhere).
    pub async fn save(pool: &Db, party: &PartyRow) -> Result<(), sqlx::Error> {
        let mut tx = pool.begin().await?;
        sqlx::query(
            "INSERT INTO parties (id, code, mission, public, modifiers) VALUES ($1, $2, $3, $4, $5) \
             ON CONFLICT (id) DO UPDATE SET code = excluded.code, mission = excluded.mission, \
             public = excluded.public, modifiers = excluded.modifiers, updated_at = now()",
        )
        .bind(party.id as i64)
        .bind(&party.code)
        .bind(&party.mission)
        .bind(party.public)
        .bind(Json(party.mods))
        .execute(&mut *tx)
        .await?;
        let users: Vec<i64> = party.members.iter().map(|m| m.user_id as i64).collect();
        sqlx::query("DELETE FROM party_members WHERE party_id = $1 AND NOT (user_id = ANY($2))")
            .bind(party.id as i64)
            .bind(&users)
            .execute(&mut *tx)
            .await?;
        for (seat, m) in party.members.iter().enumerate() {
            // A member who changed party keeps no row in the old one; the
            // join time is kept while they stay.
            sqlx::query(
                "INSERT INTO party_members (user_id, party_id, seat, character, last_seen) VALUES ($1, $2, $3, $4, $5) \
                 ON CONFLICT (user_id) DO UPDATE SET \
                 joined_at = CASE WHEN party_members.party_id = excluded.party_id THEN party_members.joined_at ELSE now() END, \
                 party_id = excluded.party_id, seat = excluded.seat, character = excluded.character, last_seen = excluded.last_seen",
            )
            .bind(m.user_id as i64)
            .bind(party.id as i64)
            .bind(seat as i16)
            .bind(Json(&m.character))
            .bind(m.last_seen)
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await
    }

    /// Removes a party and its members.
    pub async fn delete(pool: &Db, id: u32) -> Result<(), sqlx::Error> {
        sqlx::query("DELETE FROM parties WHERE id = $1").bind(id as i64).execute(pool).await?;
        Ok(())
    }
}

enum Op {
    Save(PartyRow),
    Delete(u32),
    Flush(oneshot::Sender<()>),
}

/// The lobby's write-behind: changes queue here without blocking and are
/// written one after another by a single task (see the module docs).
#[derive(Clone)]
pub struct PartyWriter {
    tx: mpsc::UnboundedSender<Op>,
}

impl PartyWriter {
    /// Starts the writer task (inside a Tokio runtime).
    pub fn spawn(pool: Db) -> Self {
        let (tx, mut rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Some(op) = rx.recv().await {
                let result = match op {
                    Op::Save(row) => PartyRepo::save(&pool, &row).await.map_err(|e| (row.id, e)),
                    Op::Delete(id) => PartyRepo::delete(&pool, id).await.map_err(|e| (id, e)),
                    Op::Flush(done) => {
                        let _ = done.send(());
                        Ok(())
                    }
                };
                if let Err((id, e)) = result {
                    // The memory stays right; the next change of the party
                    // writes it whole again.
                    error!("party {id}: could not store it: {e}");
                }
            }
        });
        Self { tx }
    }

    pub fn save(&self, row: PartyRow) {
        let _ = self.tx.send(Op::Save(row));
    }

    pub fn delete(&self, id: u32) {
        let _ = self.tx.send(Op::Delete(id));
    }

    /// Waits until everything queued before this call is written.
    pub async fn flush(&self) {
        let (done, wait) = oneshot::channel();
        if self.tx.send(Op::Flush(done)).is_ok() {
            let _ = wait.await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{testing, UserRepo};

    async fn user(pool: &Db, name: &str) -> u32 {
        UserRepo::create(pool, name, "hash").await.unwrap().id as u32
    }

    fn member(user_id: u32, name: &str, last_seen: Option<DateTime<Utc>>) -> MemberRow {
        MemberRow { user_id, character: StoredCharacter { id: user_id + 100, name: name.into(), archetype: "mage".into(), level: 7 }, last_seen }
    }

    #[tokio::test]
    async fn parties_round_trip_and_members_move() {
        let pool = testing::pool().await;
        let (a, b, c) = (user(&pool, "ann").await, user(&pool, "bob").await, user(&pool, "cid").await);
        let seen = DateTime::from_timestamp(1_700_000_000, 0).unwrap();
        let mut one = PartyRow {
            id: 4,
            code: "AB2CD".into(),
            mission: "m".into(),
            public: false,
            mods: StoredMods { tier: 2, mask: 0b101 },
            members: vec![member(b, "Bob", None), member(a, "Ann", Some(seen))],
        };
        PartyRepo::save(&pool, &one).await.unwrap();
        let two = PartyRow { id: 9, code: "ZZ9ZZ".into(), mission: "m".into(), public: true, mods: StoredMods::default(), members: vec![member(c, "Cid", None)] };
        PartyRepo::save(&pool, &two).await.unwrap();
        assert_eq!(PartyRepo::load(&pool).await.unwrap(), vec![one.clone(), two.clone()], "seat order and fields survive");

        // Ann moves to the other party; the first one is re-saved without her.
        one.members.pop();
        PartyRepo::save(&pool, &one).await.unwrap();
        let mut two = two;
        two.members.push(member(a, "Ann", None));
        PartyRepo::save(&pool, &two).await.unwrap();
        let rows = PartyRepo::load(&pool).await.unwrap();
        assert_eq!(rows[0].members.len(), 1);
        assert_eq!(rows[1].members.iter().map(|m| m.user_id).collect::<Vec<_>>(), vec![c, a]);

        PartyRepo::delete(&pool, 9).await.unwrap();
        let rows = PartyRepo::load(&pool).await.unwrap();
        assert_eq!(rows.len(), 1);
        let left: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM party_members").fetch_one(&pool).await.unwrap();
        assert_eq!(left, 1, "the members went with their party");
    }

    #[tokio::test]
    async fn the_writer_keeps_the_order_of_changes() {
        let pool = testing::pool().await;
        let a = user(&pool, "dee").await;
        let w = PartyWriter::spawn(pool.clone());
        let row = |id: u32| PartyRow { id, code: format!("C{id}"), mission: "m".into(), public: true, mods: StoredMods::default(), members: vec![member(a, "Dee", None)] };
        // Joins 1, moves to 2 (1 emptied and deleted): only 2 remains.
        w.save(row(1));
        w.save(PartyRow { members: vec![], ..row(1) });
        w.delete(1);
        w.save(row(2));
        w.flush().await;
        let rows = PartyRepo::load(&pool).await.unwrap();
        assert_eq!(rows, vec![row(2)]);
    }
}

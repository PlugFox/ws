//! The leaderboard (`GET /api/leaderboard`) and the lifetime counters it
//! ranks (`world::record`), stored on the character row.
//!
//! Every board orders by its key, descending, then the older character
//! first; each has an index on exactly that order (`migrations.rs`), so a
//! page is an index walk and a rank is an index range count. Ranks are
//! competition ranks ("1, 2, 2, 4"): equal keys share a rank, the next
//! one is its position. The level board's key is level, then experience
//! into it; the MVP board breaks ties on kills.

use crate::world::combat::Class;
use crate::world::record::Record;
use serde::Serialize;
use crate::db::Db;
use sqlx::FromRow;

/// A leaderboard: what it ranks by.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Board {
    #[default]
    Level,
    Kills,
    Mvp,
    Zeny,
}

impl Board {
    pub const ALL: [Board; 4] = [Board::Level, Board::Kills, Board::Mvp, Board::Zeny];

    /// The board of a `sort` parameter.
    pub fn parse(s: &str) -> Option<Board> {
        Self::ALL.into_iter().find(|b| b.id() == s)
    }

    pub fn id(self) -> &'static str {
        match self {
            Board::Level => "level",
            Board::Kills => "kills",
            Board::Mvp => "mvp",
            Board::Zeny => "zeny",
        }
    }

    /// The key columns, most significant first (also the index's order).
    fn key(self) -> &'static [&'static str] {
        match self {
            Board::Level => &["level", "base_exp"],
            Board::Kills => &["kills"],
            Board::Mvp => &["mvp_kills", "kills"],
            Board::Zeny => &["zeny"],
        }
    }

    fn key_of(self, e: &Entry) -> [i64; 2] {
        match self {
            Board::Level => [e.level, e.base_exp],
            Board::Kills => [e.kills, 0],
            Board::Mvp => [e.mvps, e.kills],
            Board::Zeny => [e.zeny, 0],
        }
    }

    /// Table-qualified: a bare `level` would name the BIGINT output
    /// column of [`COLUMNS`], an expression the index does not cover.
    fn order_by(self) -> String {
        let mut cols: Vec<String> = self.key().iter().map(|c| format!("characters.{c} DESC")).collect();
        cols.push("characters.id".into());
        cols.join(", ")
    }

    /// Rows strictly ahead of `key`: `(a, b) > ($1, $2)`, a row
    /// comparison on the leading columns of the board's index, so the
    /// count is an index-only range scan.
    fn ahead(self) -> String {
        let cols = self.key();
        let marks: Vec<String> = (1..=cols.len()).map(|i| format!("${i}")).collect();
        format!("({}) > ({})", cols.join(", "), marks.join(", "))
    }
}

/// One row of a board.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, FromRow)]
pub struct Entry {
    /// Competition rank on the board it was read from.
    #[sqlx(skip)]
    pub rank: i64,
    pub id: i64,
    pub name: String,
    /// Class id (`swordsman`, ...).
    #[serde(rename = "class")]
    pub archetype: String,
    /// The class's title at the level (`Crusader`).
    #[sqlx(skip)]
    pub title: String,
    pub level: i64,
    #[serde(skip)]
    pub base_exp: i64,
    pub kills: i64,
    #[sqlx(rename = "mvp_kills")]
    pub mvps: i64,
    pub zeny: i64,
    pub deaths: i64,
    #[sqlx(rename = "missions_cleared")]
    pub missions: i64,
    pub best_combo: i64,
}

/// `level` and `best_combo` are INTEGER columns, read as BIGINT like the rest.
const COLUMNS: &str =
    "id, name, archetype, level::BIGINT AS level, base_exp, kills, mvp_kills, zeny, deaths, missions_cleared, best_combo::BIGINT AS best_combo";

/// The most rows a page may ask for.
pub const MAX_LIMIT: u32 = 50;

pub struct LeaderboardRepo;

impl LeaderboardRepo {
    /// A character's stored counters (zero for an unknown id).
    pub async fn record(pool: &Db, character_id: u32) -> Result<Record, sqlx::Error> {
        let row: Option<(i64, i64, i64, i64, i64)> = sqlx::query_as(
            "SELECT kills, mvp_kills, deaths, missions_cleared, best_combo::BIGINT FROM characters WHERE id = $1",
        )
        .bind(character_id as i64)
        .fetch_optional(pool)
        .await?;
        let n = |v: i64| v.max(0) as u64;
        Ok(row.map_or_else(Record::default, |(kills, mvps, deaths, missions, best)| Record {
            kills: n(kills),
            mvps: n(mvps),
            deaths: n(deaths),
            missions: n(missions),
            best_combo: best.clamp(0, u32::MAX as i64) as u32,
        }))
    }

    /// Writes a character's counters (the flusher, with the character).
    pub async fn store_record(pool: &Db, character_id: u32, r: &Record) -> Result<(), sqlx::Error> {
        let n = |v: u64| v.min(i64::MAX as u64) as i64;
        sqlx::query("UPDATE characters SET kills = $1, mvp_kills = $2, deaths = $3, missions_cleared = $4, best_combo = $5 WHERE id = $6")
            .bind(n(r.kills))
            .bind(n(r.mvps))
            .bind(n(r.deaths))
            .bind(n(r.missions))
            .bind(r.best_combo.min(i32::MAX as u32) as i32)
            .bind(character_id as i64)
            .execute(pool)
            .await
            .map(|_| ())
    }

    /// Characters on the boards.
    pub async fn total(pool: &Db) -> Result<i64, sqlx::Error> {
        sqlx::query_scalar("SELECT COUNT(*) FROM characters").fetch_one(pool).await
    }

    /// `limit` rows of `board` from `offset`, ranked.
    pub async fn page(pool: &Db, board: Board, offset: u32, limit: u32) -> Result<Vec<Entry>, sqlx::Error> {
        let sql = format!("SELECT {COLUMNS} FROM characters ORDER BY {} LIMIT $1 OFFSET $2", board.order_by());
        let mut rows: Vec<Entry> = sqlx::query_as(&sql)
            .bind(i64::from(limit.min(MAX_LIMIT)))
            .bind(i64::from(offset))
            .fetch_all(pool)
            .await?;
        // The first row may share its rank with rows on the page before;
        // after it, a new key ranks at its position.
        let mut prev: Option<([i64; 2], i64)> = None;
        for (i, e) in rows.iter_mut().enumerate() {
            let key = board.key_of(e);
            e.rank = match prev {
                Some((k, rank)) if k == key => rank,
                Some(_) => i64::from(offset) + i as i64 + 1,
                None => Self::rank_of_key(pool, board, key).await?,
            };
            e.title = title(&e.archetype, e.level);
            prev = Some((key, e.rank));
        }
        Ok(rows)
    }

    /// One character's row on `board`, ranked (`None`: no such character).
    pub async fn entry(pool: &Db, board: Board, character_id: i64) -> Result<Option<Entry>, sqlx::Error> {
        let sql = format!("SELECT {COLUMNS} FROM characters WHERE id = $1");
        let Some(mut e) = sqlx::query_as::<_, Entry>(&sql).bind(character_id).fetch_optional(pool).await? else {
            return Ok(None);
        };
        e.rank = Self::rank_of_key(pool, board, board.key_of(&e)).await?;
        e.title = title(&e.archetype, e.level);
        Ok(Some(e))
    }

    /// 1 + the rows strictly ahead of `key`.
    async fn rank_of_key(pool: &Db, board: Board, key: [i64; 2]) -> Result<i64, sqlx::Error> {
        let sql = format!("SELECT COUNT(*) FROM characters WHERE {}", board.ahead());
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        for v in &key[..board.key().len()] {
            q = q.bind(*v);
        }
        Ok(q.fetch_one(pool).await? + 1)
    }
}

fn title(archetype: &str, level: i64) -> String {
    Class::from_id(archetype).map_or_else(|| archetype.to_string(), |c| c.title(level.clamp(1, u32::MAX as i64) as u32).to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::testing;

    async fn pool() -> Db {
        let pool = testing::pool().await;
        sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (1, 'u', 'x')").execute(&pool).await.unwrap();
        pool
    }

    /// A character with (level, exp, kills, mvps, zeny); returns its id.
    async fn add(pool: &Db, name: &str, (level, exp, kills, mvps, zeny): (i64, i64, i64, i64, i64)) -> i64 {
        sqlx::query_scalar(
            "INSERT INTO characters (user_id, name, archetype, level, base_exp, kills, mvp_kills, zeny) \
             VALUES (1, $1, 'swordsman', $2, $3, $4, $5, $6) RETURNING id",
        )
        .bind(name)
        .bind(level)
        .bind(exp)
        .bind(kills)
        .bind(mvps)
        .bind(zeny)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    fn ranked(rows: &[Entry]) -> Vec<(i64, &str)> {
        rows.iter().map(|e| (e.rank, e.name.as_str())).collect()
    }

    #[tokio::test]
    async fn boards_order_by_their_key_and_ties_share_a_rank() {
        let pool = pool().await;
        add(&pool, "Ann", (10, 50, 300, 1, 900)).await;
        add(&pool, "Bob", (12, 0, 300, 2, 100)).await;
        add(&pool, "Cid", (10, 80, 50, 2, 900)).await;
        add(&pool, "Dee", (10, 50, 0, 0, 5000)).await;

        let level = LeaderboardRepo::page(&pool, Board::Level, 0, 10).await.unwrap();
        assert_eq!(ranked(&level), [(1, "Bob"), (2, "Cid"), (3, "Ann"), (3, "Dee")], "exp breaks level ties; equal keys share");
        assert_eq!(level[0].title, "Swordsman");

        let kills = LeaderboardRepo::page(&pool, Board::Kills, 0, 10).await.unwrap();
        assert_eq!(ranked(&kills), [(1, "Ann"), (1, "Bob"), (3, "Cid"), (4, "Dee")], "the older character first in a tie");

        let mvp = LeaderboardRepo::page(&pool, Board::Mvp, 0, 10).await.unwrap();
        assert_eq!(ranked(&mvp), [(1, "Bob"), (2, "Cid"), (3, "Ann"), (4, "Dee")], "kills break MVP ties");

        let zeny = LeaderboardRepo::page(&pool, Board::Zeny, 0, 10).await.unwrap();
        assert_eq!(ranked(&zeny), [(1, "Dee"), (2, "Ann"), (2, "Cid"), (4, "Bob")]);
    }

    #[tokio::test]
    async fn pages_keep_ranks_across_their_edges() {
        let pool = pool().await;
        // Kills 90, 80, 80, 80, 70, 60, ... : a tie across pages 1 and 2.
        for (i, k) in [90, 80, 80, 80, 70, 60, 50].into_iter().enumerate() {
            add(&pool, &format!("P{i}"), (1, 0, k, 0, 0)).await;
        }
        let p1 = LeaderboardRepo::page(&pool, Board::Kills, 0, 3).await.unwrap();
        let p2 = LeaderboardRepo::page(&pool, Board::Kills, 3, 3).await.unwrap();
        let p3 = LeaderboardRepo::page(&pool, Board::Kills, 6, 3).await.unwrap();
        assert_eq!(ranked(&p1), [(1, "P0"), (2, "P1"), (2, "P2")]);
        assert_eq!(ranked(&p2), [(2, "P3"), (5, "P4"), (6, "P5")], "the tie carries over the page edge");
        assert_eq!(ranked(&p3), [(7, "P6")]);
        assert!(LeaderboardRepo::page(&pool, Board::Kills, 9, 3).await.unwrap().is_empty());
        assert_eq!(LeaderboardRepo::page(&pool, Board::Kills, 0, 500).await.unwrap().len(), 7, "the limit is capped, not refused");
        assert_eq!(LeaderboardRepo::total(&pool).await.unwrap(), 7);

        // One's own row on any board, wherever it is.
        let id = add(&pool, "Me", (1, 0, 80, 0, 0)).await;
        let me = LeaderboardRepo::entry(&pool, Board::Kills, id).await.unwrap().unwrap();
        assert_eq!((me.rank, me.name.as_str()), (2, "Me"));
        assert!(LeaderboardRepo::entry(&pool, Board::Kills, 999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn counters_round_trip() {
        let pool = pool().await;
        let id = add(&pool, "Ann", (1, 0, 0, 0, 0)).await as u32;
        assert!(LeaderboardRepo::record(&pool, id).await.unwrap().is_empty());
        let r = Record { kills: 120, mvps: 3, deaths: 4, missions: 5, best_combo: 61 };
        LeaderboardRepo::store_record(&pool, id, &r).await.unwrap();
        assert_eq!(LeaderboardRepo::record(&pool, id).await.unwrap(), r);
        let row = LeaderboardRepo::entry(&pool, Board::Mvp, id as i64).await.unwrap().unwrap();
        assert_eq!((row.kills, row.mvps, row.deaths, row.missions, row.best_combo), (120, 3, 4, 5, 61));
    }

    /// Pages and ranks of every board are index scans on its index (with
    /// sequential scans off: a handful of rows would rather be scanned).
    #[tokio::test]
    async fn boards_read_their_index() {
        let pool = pool().await;
        add(&pool, "Ann", (10, 50, 300, 1, 900)).await;
        let mut tx = pool.begin().await.unwrap();
        sqlx::query("SET LOCAL enable_seqscan = off").execute(&mut *tx).await.unwrap();
        for board in Board::ALL {
            let page = format!("EXPLAIN SELECT {COLUMNS} FROM characters ORDER BY {} LIMIT 10 OFFSET 0", board.order_by());
            let rank = format!("EXPLAIN SELECT COUNT(*) FROM characters WHERE {}", board.ahead()).replace("$1", "5").replace("$2", "5");
            for sql in [page, rank] {
                let plan: Vec<String> = sqlx::query_scalar(&sql).fetch_all(&mut *tx).await.unwrap();
                let plan = plan.join("\n");
                let index = format!("characters_lb_{}_idx", board.id());
                assert!(plan.contains(&index) && !plan.contains("Sort"), "{} board: {sql}\n{plan}", board.id());
            }
        }
    }
}

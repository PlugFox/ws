//! Settles player trades (`world::trade`) against the sessions' characters.
//!
//! Both characters' write locks are taken (lower account first, so two
//! deals can never wait on each other), the deal is re-checked against the
//! live bags, both bags are stored in one database transaction, and only
//! then do the in-memory characters change. Every other writer of a
//! character (the flusher, a displaced session's flush, a character
//! select) takes the same lock, so none of them can interleave with a
//! settlement: an item is in exactly one bag, in memory and on disk.

use super::registry::{ActiveSession, SessionRegistry};
use crate::db::character_repo::CharacterRepo;
use crate::world::trade::{exchange, Deal, Settlement, TradeError, TradeVault};
use crate::db::Db;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use tracing::{info, warn};

/// The server's [`TradeVault`]: sessions in the registry, the database.
#[derive(Clone)]
pub struct SessionVault {
    pool: Db,
    registry: SessionRegistry,
}

impl SessionVault {
    pub fn new(pool: Db, registry: SessionRegistry) -> Self {
        Self { pool, registry }
    }

    async fn settle_now(&self, deal: Deal) -> Settlement {
        let [a, b] = &deal.sides;
        if a.user_id == b.user_id {
            return Err(TradeError::Gone);
        }
        let sa = self.registry.get(a.user_id).ok_or(TradeError::Gone)?;
        let sb = self.registry.get(b.user_id).ok_or(TradeError::Gone)?;
        // Lower account first: a fixed order, so no two settlements deadlock.
        let (first, second) = if a.user_id < b.user_id { (&sa, &sb) } else { (&sb, &sa) };
        let mut g1 = first.character.write().await;
        let mut g2 = second.character.write().await;
        let (ga, gb) = if a.user_id < b.user_id { (&mut g1, &mut g2) } else { (&mut g2, &mut g1) };
        // Still the registered sessions (a displaced one is being torn
        // down), still the characters that traded.
        let current = |user: u32, s: &Arc<ActiveSession>| self.registry.get(user).is_some_and(|r| Arc::ptr_eq(&r, s));
        if !current(a.user_id, &sa) || !current(b.user_id, &sb) {
            return Err(TradeError::Gone);
        }
        let (Some(ca), Some(cb)) = (ga.as_mut(), gb.as_mut()) else { return Err(TradeError::Gone) };
        if ca.id != a.character_id || cb.id != b.character_id {
            return Err(TradeError::Gone);
        }
        let (na, nb) = exchange(&ca.bag, &cb.bag, &a.offer, &b.offer)?;
        if let Err(e) = CharacterRepo::store_trade(&self.pool, (ca.id, &na), (cb.id, &nb)).await {
            warn!("Trade {} between characters {} and {} not stored: {e}", deal.id, ca.id, cb.id);
            return Err(TradeError::Storage);
        }
        ca.set_bag(na.clone());
        cb.set_bag(nb.clone());
        info!("Trade {}: characters {} and {} exchanged", deal.id, ca.id, cb.id);
        Ok([na, nb])
    }
}

impl TradeVault for SessionVault {
    fn settle(&self, deal: Deal) -> Pin<Box<dyn Future<Output = Settlement> + Send + 'static>> {
        let me = self.clone();
        Box::pin(async move { me.settle_now(deal).await })
    }
}

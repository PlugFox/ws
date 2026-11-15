//! Arrow Change on a stage (COMBAT.md §3.2, Archer): the arrow an archer
//! has nocked sets the element of a bow's basic attack and of arrow skills
//! (`DamageFx::arrow`). Pressing the skill nocks the next arrow its level
//! opens (`cast::land`); `QUIVER_REQ` picks one by its damage type (the
//! HUD's arrow chip, and the client nocking the last one again on a new
//! stage). Changes go to the whole stage as `QUIVER` (`fight.rs`), so
//! everyone draws those arrows with the element's tip.

use super::HubInstance;
use crate::world::protocol::QUIVER_NEXT;
use std::sync::Arc;

impl HubInstance {
    /// `QUIVER_REQ`: nock the arrow of damage type id `element` (or the
    /// next one) when Arrow Change is learned and opens it. Ignored
    /// otherwise (town, another class, an arrow not open yet).
    pub fn quiver(&mut self, user_id: u32, element: u8) {
        let Some(i) = self.find(user_id) else { return };
        let book = Arc::clone(&self.skills);
        let Some(f) = self.players[i].fighter.as_mut() else { return };
        let learned = book.iter().find_map(|(k, def)| {
            let slv = f.skills.get(k as usize).copied().unwrap_or(0);
            def.quiver().filter(|_| slv > 0).map(|arrows| (arrows, slv))
        });
        let Some((arrows, slv)) = learned else { return };
        if element == QUIVER_NEXT {
            f.nock_next(arrows, slv);
            return;
        }
        let open = &arrows[..(slv as usize + 1).min(arrows.len())];
        if let Some(&arrow) = open.iter().find(|&&a| a as u8 == element) {
            f.nock(arrow);
        }
    }
}

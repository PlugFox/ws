//! Town services that are not a shop: the Hatter's recipes and an
//! Alchemist's Pharmacy (`combat::quests`), and a late join into a running
//! mission (the Kafra's party list, `lobby.rs` `join_public`).
//!
//! A craft is checked here (the recipe giver in reach, town only, no trade
//! open) and applied to the bag by the socket task ([`HubEvent::Craft`]),
//! as merchant orders are.

use super::{hub_error, Arrival, HubEvent, HubInstance, InstanceKind, MapIdx, Outgoing, PlayerProfile};
use crate::world::combat::quests;
use crate::world::map::NpcDef;
use crate::world::protocol::{RecipeInfo, ServerMsg};
use uuid::Uuid;

/// NPC kind of the Hatter: hat recipes (`combat::quests::HATTER`).
pub const HATTER: &str = "hatter";
/// The "NPC" id of an Alchemist's Pharmacy: not an NPC but a skill, opened
/// (`npc_interact`) and brewed at (`QUEST` craft) anywhere in town by a
/// player who learned it (`combat::quests::PHARMACY`).
pub const PHARMACY: &str = "pharmacy";

/// A late joiner lands this far (cells) from the party member they join.
const BESIDE: [f32; 3] = [1.5, 2.5, 3.5];

/// The recipe giver's offer (`QUESTS` recipes).
pub(super) fn recipes_msg(n: &NpcDef) -> ServerMsg {
    book_msg(&n.id, &n.name, quests::HATTER.iter())
}

fn book_msg<'a>(npc: &str, name: &str, recipes: impl Iterator<Item = &'a quests::Recipe>) -> ServerMsg {
    ServerMsg::Recipes {
        npc: npc.into(),
        name: name.into(),
        recipes: recipes
            .map(|r| RecipeInfo {
                id: r.id.into(),
                item: r.item.into(),
                zeny: r.zeny,
                needs: r.needs.iter().map(|&(i, n)| (i.to_string(), n)).collect(),
            })
            .collect(),
    }
}

impl HubInstance {
    /// The NPC `id` on `map`; with the `GAME_DEV` hooks (`missions_anywhere`)
    /// on any map of the world, so the e2e `?shop=` opens a merchant that
    /// keeps shop indoors from the town square.
    pub(super) fn npc_here(&self, map: MapIdx, id: &str) -> Option<&NpcDef> {
        let here = self.world.map(map).manifest.npcs.iter().find(|n| n.id == id);
        if here.is_some() || !self.config.missions_anywhere {
            return here;
        }
        (0..self.world.map_count()).flat_map(|m| self.world.map(m as MapIdx).manifest.npcs.iter()).find(|n| n.id == id)
    }

    /// The Pharmacy window of `user_id` (`npc_interact` with
    /// [`PHARMACY`]): in town, with the skill learned; the recipes its
    /// level opens.
    pub(super) fn open_pharmacy(&mut self, user_id: u32, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        let level = self.skills.pharmacy(&self.players[i].profile.loadout);
        let reply = if level == 0 {
            hub_error("not_learned", "Learn Pharmacy first.")
        } else if !matches!(self.kind, InstanceKind::Hub { .. }) {
            hub_error("not_in_hub", "Brewing is done in town.")
        } else {
            book_msg(PHARMACY, "Pharmacy", quests::PHARMACY.iter().filter(|r| r.level <= level))
        };
        out.push((user_id, HubEvent::Control(reply)));
    }

    /// `recipe` at the recipe giver `npc` (or the player's own Pharmacy):
    /// in town, standing at it, not trading. A good one goes to the socket
    /// task as [`HubEvent::Craft`]; a Pharmacy at its top level rolls here
    /// for a double brew.
    pub fn craft(&mut self, user_id: u32, npc: &str, recipe: &str, out: &mut Outgoing) {
        let Some(i) = self.find(user_id) else { return };
        if !matches!(self.kind, InstanceKind::Hub { .. }) {
            out.push((user_id, HubEvent::Control(hub_error("not_in_hub", "Crafting is done in town."))));
            return;
        }
        if self.is_trading(user_id) {
            out.push((user_id, HubEvent::Control(hub_error("trade_busy", "Finish your trade first."))));
            return;
        }
        if npc == PHARMACY {
            let level = self.skills.pharmacy(&self.players[i].profile.loadout);
            let event = match quests::PHARMACY.iter().find(|r| r.id == recipe) {
                _ if level == 0 => HubEvent::Control(hub_error("not_learned", "Learn Pharmacy first.")),
                None => HubEvent::Control(hub_error("unknown_recipe", "The Pharmacy has no such recipe.")),
                Some(r) if r.level > level => HubEvent::Control(hub_error("level_too_low", format!("Needs Pharmacy level {}.", r.level))),
                Some(r) => {
                    let double = level >= quests::DOUBLE_LEVEL && self.dice.die(100) <= quests::DOUBLE_CHANCE;
                    HubEvent::Craft { recipe: r.id, count: 1 + double as u32 }
                }
            };
            out.push((user_id, event));
            return;
        }
        let p = &self.players[i];
        let event = match self.npc_here(p.map, npc).filter(|n| n.kind == HATTER) {
            None => HubEvent::Control(hub_error("no_such_npc", format!("no recipe giver {npc} here"))),
            Some(n) if !self.at_merchant(n, p.x, p.y) => HubEvent::Control(hub_error("npc_too_far", format!("walk closer to {}", n.name))),
            Some(n) => match quests::HATTER.iter().find(|r| r.id == recipe) {
                None => HubEvent::Control(hub_error("unknown_recipe", format!("{} has no such recipe", n.name))),
                Some(r) => HubEvent::Craft { recipe: r.id, count: 1 },
            },
        };
        out.push((user_id, event));
    }

    /// Adds a player to a mission already under way (a late join from the
    /// party list): next to a party member who is up and connected, else
    /// at the spawn. Everything else is a normal [`Self::join`]: a fresh
    /// fighter, the stage, the brief, the clock, the fog; the boss bar
    /// follows with its next check.
    pub fn join_late(&mut self, profile: PlayerProfile, session_id: Uuid, epoch: u16, out: &mut Outgoing) {
        let arrival = self.beside_member(profile.user_id);
        self.join(profile, session_id, arrival, epoch, out);
        if let Some(b) = self.boss.as_mut() {
            b.resend();
        }
    }

    /// A free walkable cell near the first member who is up and online
    /// (on their map); their own spot when it is cramped.
    fn beside_member(&self, user_id: u32) -> Option<Arrival> {
        let buddy = self
            .players
            .iter()
            .find(|p| p.profile.user_id != user_id && !p.link_dead && p.fighter.as_ref().is_none_or(|f| f.is_up()))?;
        let map = self.world.map(buddy.map);
        let (bx, by) = (buddy.x, buddy.y);
        // Eight directions on each ring, starting away from the buddy's facing.
        let spot = BESIDE.iter().find_map(|&r| {
            (0..8).find_map(|k| {
                let a = (k as f32 + buddy.facing as f32 + 4.0) * std::f32::consts::FRAC_PI_4;
                let (x, y) = ((bx + a.sin() * r).floor() + 0.5, (by + a.cos() * r).floor() + 0.5);
                map.walk.is_walkable(x, y).then_some((x, y))
            })
        });
        let (x, y) = spot.unwrap_or((bx, by));
        Some(Arrival { map: map.id().to_string(), x, y })
    }
}

#[cfg(test)]
mod tests {
    use std::assert_matches;
    use super::*;
    use crate::world::hub::test_support::map;
    use crate::world::hub::{HubSimConfig, HubWorld};
    use crate::world::map::{CameraConstraint, LocationKind};
    use std::sync::Arc;

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile {
            user_id,
            character_id: 100 + user_id,
            name: format!("p{user_id}"),
            archetype: "novice".into(),
            look: Default::default(),
            level: 1,
            exp: 0,
            weapon: String::new(),
            loadout: Default::default(),
            bag: Default::default(),
            homunculus: None,
        }
    }

    /// A 30×7 town, the spawn in the middle, the Hatter at `at`.
    fn town_with_hatter(at: (u32, u32), kind: LocationKind) -> HubInstance {
        let wall = "#".repeat(30);
        let row = format!("#{}#", ".".repeat(28));
        let rows: Vec<&str> = [wall.as_str(), &row, &row, &row, &row, &row, wall.as_str()].to_vec();
        let mut town = map("town", kind, CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 }, &rows, vec![]);
        town.manifest.npcs.push(NpcDef {
            id: "hatter".into(),
            sprite: "x".into(),
            name: "Hatter".into(),
            kind: HATTER.into(),
            shop: String::new(),
            x: at.0,
            y: at.1,
            facing: 4,
            interact_radius: 3.0,
        });
        let world = Arc::new(HubWorld::new(vec![town], "town").unwrap());
        match kind {
            LocationKind::Field => HubInstance::stage(1, "m", world, HubSimConfig::default()),
            _ => HubInstance::new(1, world, HubSimConfig::default()),
        }
    }

    fn mine(out: &Outgoing, user: u32) -> Vec<&HubEvent> {
        out.iter().filter(|(u, _)| *u == user).map(|(_, e)| e).collect()
    }

    fn error_code(out: &Outgoing, user: u32) -> Option<String> {
        mine(out, user).into_iter().find_map(|e| match e {
            HubEvent::Control(ServerMsg::HubError { code, .. }) => Some(code.clone()),
            _ => None,
        })
    }

    #[test]
    fn the_hatter_lists_recipes_and_crafts_only_within_reach() {
        let mut out = Vec::new();
        // Far: the Hatter in a corner, the player on the spawn mid-map.
        let mut hub = town_with_hatter((1, 1), LocationKind::Hub);
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.craft(1, "hatter", "ribbon", &mut out);
        assert_eq!(error_code(&out, 1).as_deref(), Some("npc_too_far"));

        let mut hub = town_with_hatter((15, 3), LocationKind::Hub);
        hub.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        hub.interact(1, "hatter", &mut out);
        match mine(&out, 1)[..] {
            [HubEvent::Control(ServerMsg::Recipes { npc, recipes, .. })] => {
                assert_eq!(npc, "hatter");
                assert_eq!(recipes.len(), quests::HATTER.len());
                assert_eq!(recipes[0].needs[0], ("jellopy".to_string(), 10));
            }
            ref other => panic!("{other:?}"),
        }
        out.clear();
        hub.craft(1, "hatter", "ribbon", &mut out);
        assert_matches!(mine(&out, 1)[..], [HubEvent::Craft { recipe: "ribbon", count: 1 }], "{out:?}");
        out.clear();
        hub.craft(1, "hatter", "crown", &mut out);
        assert_eq!(error_code(&out, 1).as_deref(), Some("unknown_recipe"));
        out.clear();
        hub.craft(1, "nobody", "ribbon", &mut out);
        assert_eq!(error_code(&out, 1).as_deref(), Some("no_such_npc"));
    }

    #[test]
    fn nothing_is_crafted_on_a_mission() {
        let mut out = Vec::new();
        let mut stage = town_with_hatter((15, 3), LocationKind::Field);
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        stage.craft(1, "hatter", "ribbon", &mut out);
        assert_eq!(error_code(&out, 1).as_deref(), Some("not_in_hub"));
    }

    #[test]
    fn a_late_joiner_lands_beside_a_member_who_is_up() {
        let mut out = Vec::new();
        let mut stage = town_with_hatter((1, 1), LocationKind::Field);
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        stage.join(profile(2), Uuid::nil(), None, 0, &mut out);
        // The party walked off east; 2 dropped (link-dead).
        let (_, sx, sy) = stage.position_of(1).unwrap();
        let i = stage.find(1).unwrap();
        (stage.players[i].x, stage.players[i].y) = (24.5, 2.5);
        stage.detach(2, Uuid::nil(), &mut out);
        out.clear();
        stage.join_late(profile(3), Uuid::nil(), 5, &mut out);
        let (map, x, y) = stage.position_of(3).unwrap();
        assert_eq!(map, "town");
        let d = (x - 24.5).hypot(y - 2.5);
        assert!((1.0..=4.5).contains(&d), "beside 1 at ({x},{y}), {d} away (spawn {sx},{sy})");
        assert!(stage.world.map(0).walk.is_walkable(x, y));
        // The newcomer gets the stage like any arrival: joined, the map,
        // its fighter's vitals.
        let theirs = mine(&out, 3);
        assert_matches!(theirs.first(), Some(HubEvent::Control(ServerMsg::StageJoined { .. })), "{theirs:?}");
        assert!(theirs.iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::MapEnter { .. }))));
        assert!(stage.players[stage.find(3).unwrap()].fighter.as_ref().is_none_or(|f| f.is_up() && f.hp == f.max_hp));
        // Everyone else sees them arrive.
        assert!(mine(&out, 1).iter().any(|e| matches!(e, HubEvent::Control(ServerMsg::PlayerAppeared { .. }))));
    }

    /// A field without a walk-out portal (Prontera's bridge portal is
    /// gone) still sends everyone home when the run is over.
    #[test]
    fn a_stage_without_a_walk_out_portal_sends_everyone_to_town() {
        let mut out = Vec::new();
        let mut stage = town_with_hatter((1, 1), LocationKind::Field);
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        out.clear();
        let entry = stage.world.entry();
        stage.leave_stage(entry, &mut out);
        assert!(out.iter().any(|(u, e)| *u == 1 && matches!(e, HubEvent::Transfer { target_map, .. } if target_map.is_empty())), "{out:?}");
        assert_eq!(stage.population(), 0);
    }

    #[test]
    fn a_late_joiner_with_nobody_up_starts_on_the_spawn() {
        let mut out = Vec::new();
        let mut stage = town_with_hatter((1, 1), LocationKind::Field);
        stage.join_late(profile(3), Uuid::nil(), 0, &mut out);
        let (_, x, y) = stage.position_of(3).unwrap();
        assert_eq!((x, y), stage.world.map(stage.world.entry()).spawn);
    }
}

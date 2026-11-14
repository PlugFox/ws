//! The overtime poison fog (COMBAT.md §4.7): a run dragged out past its
//! boss closes in like a battle-royale zone.
//!
//! When the mission's [`FogRules`] say so (a boss on the field for a while,
//! or a fixed point of the run at the latest), a poison fog rises at the
//! edge of the reachable field and its safe circle shrinks linearly,
//! concentric, onto a walkable open spot between the map's centre and the
//! party, then holds there until the run ends. The server owns the circle
//! and sends it once (`FOG`); clients draw it from the run clock.
//!
//! Every second a player spends outside the circle adds one second of
//! exposure (a second inside takes one off). A standing player outside
//! loses `2 % + 1 % per 30 s the fog has been up + 1 % per second of
//! exposure` of their maximum HP, straight off HP: no armour, resistance,
//! immunity, temporary HP or ward stops it. At [`DEATH_SECS`] of exposure
//! anyone, standing or downed, dies outright and stays dead for the rest of
//! the run. The downed take no damage but their exposure keeps counting.
//! Monsters live in it unharmed: the fog is there to end the run, not to
//! clear the field.

use super::fight::{damage_type_bits, Vital};
use super::mission::Outcome;
use super::{HubEvent, HubInstance, MapIdx, Outgoing};
use crate::world::combat::DamageType;
use crate::world::map::MapData;
use crate::world::missions::FogRules;
use crate::world::protocol::{HitEvent, ServerMsg};
use tracing::info;

/// Net seconds inside the fog that kill.
pub(super) const DEATH_SECS: u8 = 30;
/// Damage per second outside, in percent of maximum HP: the base ...
const BASE_PCT: u32 = 2;
/// ... one more per this many seconds the fog has been up ...
const AGE_SECS_PER_PCT: f32 = 30.0;
/// ... and this much per second of the player's exposure.
const EXPOSURE_PCT: u32 = 1;
/// The safe spot lies this far from the map's spawn toward the party.
const PARTY_PULL: f32 = 0.35;
/// The safe spot has open ground this far around it (cells, a square).
const OPEN_CELLS: i32 = 3;

/// The safe circle: concentric, closing linearly from `from_radius` at
/// `start_secs` to `to_radius` over `shrink_secs` of the run, then holding.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Circle {
    pub x: f32,
    pub y: f32,
    pub from_radius: f32,
    pub to_radius: f32,
    pub start_secs: f32,
    pub shrink_secs: f32,
}

impl Circle {
    /// Safe radius (cells) at `secs` into the run.
    pub fn radius_at(&self, secs: f32) -> f32 {
        let t = if self.shrink_secs > 0.0 { ((secs - self.start_secs) / self.shrink_secs).clamp(0.0, 1.0) } else { 1.0 };
        self.from_radius + (self.to_radius - self.from_radius) * t
    }

    /// Whether `(x, y)` is in the fog at `secs`.
    pub fn outside(&self, x: f32, y: f32, secs: f32) -> bool {
        let r = self.radius_at(secs);
        let (dx, dy) = (x - self.x, y - self.y);
        dx * dx + dy * dy > r * r
    }

    fn msg(&self, begun: bool, now: f32) -> ServerMsg {
        let ms = |s: f32| (s.max(0.0) * 1000.0).round() as u32;
        ServerMsg::Fog {
            begun,
            now_ms: ms(now),
            start_ms: ms(self.start_secs),
            shrink_ms: ms(self.shrink_secs),
            x: self.x,
            y: self.y,
            from_radius: self.from_radius,
            to_radius: self.to_radius,
        }
    }
}

/// The fog of one stage: its rules, when the boss was first seen, and the
/// circle once it rose.
#[derive(Debug, Default)]
pub(super) struct Fog {
    rules: Option<FogRules>,
    boss_since: Option<f32>,
    pub(super) circle: Option<Circle>,
}

impl Fog {
    pub fn new(rules: Option<FogRules>) -> Self {
        Self { rules, boss_since: None, circle: None }
    }

    /// Whether the fog is due at `now` (run seconds); remembers when a boss
    /// was first `boss_present`.
    fn due(&mut self, now: f32, boss_present: bool) -> bool {
        let Some(rules) = self.rules else { return false };
        if boss_present {
            self.boss_since.get_or_insert(now);
        }
        let at = self.boss_since.map_or(rules.latest_secs, |t| (t + rules.after_boss_secs).min(rules.latest_secs));
        now >= at
    }
}

/// Damage of one second outside (percent of maximum HP): the base, the
/// fog's age and the player's exposure (this second included).
pub(super) fn toll_pct(age_secs: f32, exposure: u8) -> u32 {
    BASE_PCT + (age_secs.max(0.0) / AGE_SECS_PER_PCT) as u32 + EXPOSURE_PCT * u32::from(exposure)
}

/// `pct` percent of `max_hp`, rounded up, at least 1.
fn toll(max_hp: i32, pct: u32) -> i32 {
    ((i64::from(max_hp.max(1)) * i64::from(pct) + 99) / 100).max(1) as i32
}

/// The safe circle's centre and starting radius: the reachable cell with
/// open ground around it nearest to a point between the map's spawn and the
/// `party` (any reachable cell if none is open), and the distance to the
/// farthest reachable cell (so the fog starts at the very edge).
pub(super) fn safe_circle(map: &MapData, party: &[(f32, f32)]) -> (f32, f32, f32) {
    let (w, h) = (map.walk.width() as i32, map.walk.height() as i32);
    let (sx, sy) = map.spawn;
    let (tx, ty) = if party.is_empty() {
        (sx, sy)
    } else {
        let n = party.len() as f32;
        let (px, py) = party.iter().fold((0.0, 0.0), |a, p| (a.0 + p.0, a.1 + p.1));
        (sx + (px / n - sx) * PARTY_PULL, sy + (py / n - sy) * PARTY_PULL)
    };
    let reach = |x: i32, y: i32| x >= 0 && y >= 0 && map.is_reachable_cell(x as u32, y as u32);
    let open = |x: i32, y: i32| (-OPEN_CELLS..=OPEN_CELLS).all(|dy| (-OPEN_CELLS..=OPEN_CELLS).all(|dx| reach(x + dx, y + dy)));
    let d2 = |x: i32, y: i32, cx: f32, cy: f32| (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2);
    let mut best: Option<(f32, i32, i32)> = None;
    let mut near: Option<(f32, i32, i32)> = None;
    for y in 0..h {
        for x in 0..w {
            if !reach(x, y) {
                continue;
            }
            let d = d2(x, y, tx, ty);
            if near.is_none_or(|b| d < b.0) {
                near = Some((d, x, y));
            }
            // The open test only runs for a nearer candidate.
            if best.is_none_or(|b| d < b.0) && open(x, y) {
                best = Some((d, x, y));
            }
        }
    }
    let Some((_, cx, cy)) = best.or(near) else { return (sx, sy, 0.0) };
    let (cx, cy) = (cx as f32 + 0.5, cy as f32 + 0.5);
    let mut far = 0.0f32;
    for y in 0..h {
        for x in 0..w {
            if reach(x, y) {
                far = far.max(d2(x, y, cx, cy));
            }
        }
    }
    // To the far corner of the farthest cell.
    (cx, cy, far.sqrt() + 0.75)
}

impl HubInstance {
    /// Once per tick in the combat step: the fog rises when due, then takes
    /// its toll once a second (staggered by player) from whoever stands
    /// outside the circle. `hits` shows the damage as poison.
    pub(super) fn fog_step(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        if self.outcome != Outcome::Running {
            return;
        }
        let Some(horde) = self.horde.as_ref() else { return };
        let map = horde.map();
        let now = horde.director_elapsed();
        let Some(circle) = self.fog_rise(map, now, out) else { return };
        let hz = self.config.tick_hz.max(1);
        let tick = self.tick;
        let age = now - circle.start_secs;
        for p in self.players.iter_mut().filter(|p| p.map == map) {
            let uid = p.profile.user_id;
            let Some(f) = p.fighter.as_mut() else { continue };
            if !(tick + uid).is_multiple_of(hz) {
                continue;
            }
            let outside = circle.outside(p.x, p.y, now);
            match f.state {
                Vital::Dead => {}
                _ if !outside => f.fog = f.fog.saturating_sub(1),
                Vital::Up => {
                    f.fog = f.fog.saturating_add(1).min(DEATH_SECS);
                    // Straight off HP: no armour, resistance, immunity,
                    // temporary HP, ward or Energy Coat.
                    let amount = toll(f.max_hp, toll_pct(age, f.fog));
                    f.hp -= amount;
                    f.quiet = 0;
                    hits.push(HitEvent { target: p.profile.character_id, amount: amount as u32, flags: damage_type_bits(DamageType::Poison) });
                    f.fall_if_spent(hz as f32);
                    if f.fog >= DEATH_SECS {
                        f.perish(hz as f32);
                    }
                }
                Vital::Downed { .. } => {
                    f.fog = f.fog.saturating_add(1).min(DEATH_SECS);
                    if f.fog >= DEATH_SECS {
                        f.perish(hz as f32);
                    }
                }
            }
            if f.fog != f.fog_sent {
                f.fog_sent = f.fog;
                out.push((uid, HubEvent::Control(ServerMsg::FogExposure { secs: f.fog, death_secs: DEATH_SECS })));
            }
        }
    }

    /// The circle, raising the fog when it is due (`FOG` op 0 to the map).
    fn fog_rise(&mut self, map: MapIdx, now: f32, out: &mut Outgoing) -> Option<Circle> {
        if let Some(c) = self.fog.circle {
            return Some(c);
        }
        if !self.fog.due(now, self.boss.is_some()) {
            return None;
        }
        let rules = self.fog.rules?;
        let on_map = || self.players.iter().filter(|p| p.map == map);
        let mut party: Vec<(f32, f32)> =
            on_map().filter(|p| p.fighter.as_ref().is_some_and(|f| f.is_up())).map(|p| (p.x, p.y)).collect();
        if party.is_empty() {
            party = on_map().map(|p| (p.x, p.y)).collect();
        }
        let (x, y, edge) = safe_circle(self.world.map(map), &party);
        let c = Circle {
            x,
            y,
            from_radius: edge.max(rules.final_radius),
            to_radius: rules.final_radius,
            start_secs: now,
            shrink_secs: rules.shrink_secs,
        };
        self.fog.circle = Some(c);
        info!("{}: poison fog rises at {now:.0} s around ({x:.1}, {y:.1}), {:.0} → {:.0} cells", self.name, c.from_radius, c.to_radius);
        self.broadcast(map, &c.msg(true, now), out);
        Some(c)
    }

    /// The fog's circle and the run clock, once it has risen.
    pub(super) fn fog_now(&self) -> Option<(Circle, f32)> {
        Some((self.fog.circle?, self.horde.as_ref()?.director_elapsed()))
    }

    /// A player joining a fogged stage: the circle and their exposure.
    pub(super) fn announce_fog(&self, idx: usize, out: &mut Outgoing) {
        let (Some(c), Some(horde)) = (self.fog.circle, self.horde.as_ref()) else { return };
        let p = &self.players[idx];
        if p.map != horde.map() {
            return;
        }
        let uid = p.profile.user_id;
        out.push((uid, HubEvent::Control(c.msg(false, horde.director_elapsed()))));
        if let Some(f) = p.fighter.as_ref().filter(|f| f.fog > 0) {
            out.push((uid, HubEvent::Control(ServerMsg::FogExposure { secs: f.fog, death_secs: DEATH_SECS })));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::map;
    use super::super::{HubSimConfig, HubWorld, PlayerProfile};
    use super::*;
    use crate::world::combat::{self, Mods};
    use crate::world::horde::catalog::{test_def, MonsterCatalog};
    use crate::world::horde::{Horde, HordeConfig};
    use crate::world::map::{CameraConstraint, LocationKind};
    use crate::world::protocol::Look;
    use std::sync::Arc;
    use uuid::Uuid;

    const RULES: FogRules = FogRules { after_boss_secs: 120.0, latest_secs: 720.0, shrink_secs: 150.0, final_radius: 3.0 };

    fn profile(user_id: u32) -> PlayerProfile {
        PlayerProfile {
            user_id,
            character_id: 100 + user_id,
            name: format!("p{user_id}"),
            archetype: "swordsman".into(),
            look: Look::default(),
            level: 10,
            exp: 0,
            weapon: String::new(),
            loadout: Default::default(),
            bag: Default::default(),
        }
    }

    /// A 40 × 21 field (walls around) with a sealed pocket in the corner
    /// and a boss kind; player 1 joins. The director is off: tests set the
    /// run clock.
    fn fog_stage() -> HubInstance {
        let wall = "#".repeat(42);
        let row = format!("#{}#", ".".repeat(40));
        // A pocket (unreachable) at the top right.
        let pocket = format!("#{}#.#", ".".repeat(38));
        let sill = format!("#{}###", ".".repeat(38));
        let mut rows = vec![wall.clone(), pocket, sill];
        rows.extend(std::iter::repeat_n(row, 19));
        rows.push(wall);
        let rows: Vec<&str> = rows.iter().map(String::as_str).collect();
        let field = map(
            "field",
            LocationKind::Field,
            CameraConstraint::Free { pitch: -45.0, min_zoom: 10.0, max_zoom: 40.0, default_zoom: 20.0 },
            &rows,
            vec![],
        );
        let world = Arc::new(HubWorld::new(vec![field], "field").unwrap());
        let catalog = Arc::new(
            MonsterCatalog::new(vec![test_def("blob", 2.0, 1, 0.0, false), test_def("king", 2.0, 50, 0.0, true)]).unwrap(),
        );
        let cfg = HordeConfig { light_radius: 12.0, director_enabled: false, seed: Some(1), ..HordeConfig::default() };
        let horde = Horde::new(Arc::clone(&world), catalog, cfg);
        let mut stage = HubInstance::stage(1, "m1", world, HubSimConfig { residents: 0, ..Default::default() }).with_horde(horde);
        stage.fog = Fog::new(Some(RULES));
        stage.dice = combat::Rng::new(9);
        let mut out = Vec::new();
        stage.join(profile(1), Uuid::nil(), None, 0, &mut out);
        stage
    }

    fn clock(stage: &mut HubInstance, secs: f32) {
        stage.horde_mut().unwrap().set_director_elapsed(secs);
    }

    fn put(stage: &mut HubInstance, user: u32, x: f32, y: f32) {
        let i = stage.find(user).unwrap();
        (stage.players[i].x, stage.players[i].y) = (x, y);
    }

    fn fighter(stage: &mut HubInstance, user: u32) -> &mut super::super::Fighter {
        let i = stage.find(user).unwrap();
        stage.players[i].fighter.as_mut().unwrap()
    }

    fn fogs(out: &Outgoing, user: u32) -> Vec<ServerMsg> {
        out.iter()
            .filter(|(u, _)| *u == user)
            .filter_map(|(_, e)| match e {
                HubEvent::Control(m @ (ServerMsg::Fog { .. } | ServerMsg::FogExposure { .. })) => Some(m.clone()),
                _ => None,
            })
            .collect()
    }

    /// Poison hits on player 1 (entity 101).
    fn poison_hits(out: &Outgoing) -> Vec<u32> {
        out.iter()
            .filter_map(|(u, e)| match e {
                HubEvent::Control(ServerMsg::Combat { hits, .. }) if *u == 1 => Some(hits.clone()),
                _ => None,
            })
            .flatten()
            .filter(|h| h.target == 101 && h.flags >> 4 == damage_type_bits(DamageType::Poison) >> 4)
            .map(|h| h.amount)
            .collect()
    }

    fn run(stage: &mut HubInstance, ticks: u32, out: &mut Outgoing) {
        for _ in 0..ticks {
            stage.tick(out);
        }
    }

    /// The circle closes quickly onto a point in the far west corner, for
    /// the toll tests: x 4.5 is safe, x 30 is in the fog.
    fn fog_up_west(stage: &mut HubInstance) {
        stage.fog.circle =
            Some(Circle { x: 4.5, y: 10.5, from_radius: 3.0, to_radius: 3.0, start_secs: 0.0, shrink_secs: 0.0 });
    }

    #[test]
    fn the_fog_rises_at_the_latest_point_without_a_boss() {
        let mut stage = fog_stage();
        let mut out = Vec::new();
        clock(&mut stage, 719.0);
        run(&mut stage, 40, &mut out);
        assert!(fogs(&out, 1).is_empty(), "not yet");
        clock(&mut stage, 720.0);
        stage.tick(&mut out);
        let begun: Vec<_> = fogs(&out, 1).into_iter().filter(|m| matches!(m, ServerMsg::Fog { begun: true, .. })).collect();
        assert_eq!(begun.len(), 1, "one warning to the map");
        let ServerMsg::Fog { now_ms, start_ms, shrink_ms, x, y, from_radius, to_radius, .. } = begun[0] else { unreachable!() };
        assert_eq!((now_ms, start_ms, shrink_ms, to_radius), (720_000, 720_000, 150_000, 3.0));
        // Centre: reachable with open ground around it; the start radius
        // takes in every reachable cell, and no more than that.
        let m = stage.world.map(stage.world.entry());
        assert!(m.is_reachable_cell(x as u32, y as u32));
        assert!(!m.is_reachable_cell(40, 21), "the sealed pocket");
        for dy in -OPEN_CELLS..=OPEN_CELLS {
            for dx in -OPEN_CELLS..=OPEN_CELLS {
                assert!(m.is_reachable_cell((x as i32 + dx) as u32, (y as i32 + dy) as u32));
            }
        }
        let mut far = 0.0f32;
        for cy in 0..23 {
            for cx in 0..42 {
                if m.is_reachable_cell(cx, cy) {
                    far = far.max(((cx as f32 + 0.5 - x).powi(2) + (cy as f32 + 0.5 - y).powi(2)).sqrt());
                }
            }
        }
        assert!(from_radius >= far && from_radius <= far + 1.0, "{from_radius} vs {far}");
        out.clear();
        run(&mut stage, 40, &mut out);
        assert!(!fogs(&out, 1).iter().any(|m| matches!(m, ServerMsg::Fog { .. })), "sent once");
    }

    #[test]
    fn a_boss_on_the_field_brings_the_fog_two_minutes_later() {
        let mut stage = fog_stage();
        let mut out = Vec::new();
        clock(&mut stage, 300.0);
        let king = stage.horde().unwrap().catalog().defs().iter().position(|d| d.boss).unwrap() as u8;
        stage.horde_mut().unwrap().spawn(king, 35.5, 15.5).unwrap();
        run(&mut stage, 25, &mut out);
        assert!(stage.boss.is_some(), "the boss bar found it");
        clock(&mut stage, 419.0);
        run(&mut stage, 5, &mut out);
        assert!(stage.fog.circle.is_none());
        clock(&mut stage, 420.0);
        stage.tick(&mut out);
        assert_eq!(stage.fog.circle.map(|c| c.start_secs), Some(420.0));
    }

    /// Poring Island (when the asset submodule is baked): the safe spot is
    /// open island ground and the fog starts at the shore.
    #[test]
    fn poring_island_has_a_reachable_safe_spot() {
        let dist = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/dist");
        let Ok(m) = MapData::load(&dist, "prt_fild08") else { return };
        let t = std::time::Instant::now();
        let (x, y, r) = safe_circle(&m, &[]);
        let party = safe_circle(&m, &[(m.spawn.0 + 60.0, m.spawn.1)]);
        eprintln!("prt_fild08: spawn {:?}, safe spot ({x}, {y}), edge {r} cells; party east {party:?}; {:?}", m.spawn, t.elapsed());
        assert!(m.is_reachable_cell(x as u32, y as u32) && m.is_reachable_cell(party.0 as u32, party.1 as u32));
        assert!(party.0 > x, "pulled toward the party");
        assert!(r > 40.0 && r < 300.0);
    }

    #[test]
    fn the_circle_shrinks_linearly_then_holds() {
        let c = Circle { x: 10.0, y: 10.0, from_radius: 160.0, to_radius: 10.0, start_secs: 720.0, shrink_secs: 150.0 };
        assert_eq!(c.radius_at(700.0), 160.0);
        assert_eq!(c.radius_at(720.0), 160.0);
        assert_eq!(c.radius_at(795.0), 85.0);
        assert_eq!(c.radius_at(870.0), 10.0);
        assert_eq!(c.radius_at(899.0), 10.0);
        assert!(!c.outside(19.0, 10.0, 899.0));
        assert!(c.outside(20.5, 10.0, 899.0));
        assert!(!c.outside(90.0, 10.0, 795.0));
    }

    #[test]
    fn the_toll_ramps_and_ignores_armour_resistance_and_temporary_hp() {
        let mut stage = fog_stage();
        fog_up_west(&mut stage);
        put(&mut stage, 1, 30.5, 10.5);
        let f = fighter(&mut stage, 1);
        f.max_hp = 1000;
        f.hp = 1000;
        f.ac = 99;
        f.buff(0xFFF0, u32::MAX, Mods::default(), 500);
        f.set_passive(Mods { resist: 1.0, reduce: 100, mana_shield: true, block: 100, ..Mods::default() });
        let hz = stage.config.tick_hz;
        let mut out = Vec::new();
        run(&mut stage, hz * 4, &mut out);
        // 2 % base + 1 % per second of exposure: 3, 4, 5, 6 % of 1000.
        assert_eq!(poison_hits(&out), vec![30, 40, 50, 60]);
        let f = fighter(&mut stage, 1);
        assert_eq!((f.hp, f.temp_hp, f.fog), (1000 - 180, 500, 4));
        let exposure: Vec<_> = fogs(&out, 1).into_iter().filter_map(|m| match m {
            ServerMsg::FogExposure { secs, death_secs } => Some((secs, death_secs)),
            _ => None,
        }).collect();
        assert_eq!(exposure, vec![(1, 30), (2, 30), (3, 30), (4, 30)]);
        // Back inside: no toll, the exposure drains a second per second.
        put(&mut stage, 1, 4.5, 10.5);
        out.clear();
        run(&mut stage, hz * 2, &mut out);
        assert!(poison_hits(&out).is_empty());
        assert_eq!(fighter(&mut stage, 1).fog, 2);
        // The longer the fog has been up, the harder it bites.
        assert_eq!(toll_pct(0.0, 1), 3);
        assert_eq!(toll_pct(150.0, 1), 8);
        assert_eq!(toll(1000, 8), 80);
        assert_eq!(toll(7, 3), 1, "rounded up, at least 1");
    }

    #[test]
    fn too_long_in_the_fog_kills_outright_for_the_rest_of_the_run() {
        let mut stage = fog_stage();
        fog_up_west(&mut stage);
        put(&mut stage, 1, 30.5, 10.5);
        let hz = stage.config.tick_hz;
        let mut out = Vec::new();
        // Healed to full every second (the ramp adds up to 525 % of the
        // HP over 30 s): only the exposure can kill.
        for _ in 0..29 {
            let f = fighter(&mut stage, 1);
            f.hp = f.max_hp;
            run(&mut stage, hz, &mut out);
            assert!(fighter(&mut stage, 1).is_up());
        }
        let f = fighter(&mut stage, 1);
        f.hp = f.max_hp;
        run(&mut stage, hz, &mut out);
        let f = fighter(&mut stage, 1);
        assert_eq!((f.state, f.fog, f.fog_dead, f.hp), (Vital::Dead, DEATH_SECS, true, 0));
        assert!(matches!(stage.outcome, Outcome::Over { .. }), "solo: the run is lost");
    }

    #[test]
    fn the_downed_take_no_toll_but_their_exposure_still_kills() {
        let mut stage = fog_stage();
        let mut out = Vec::new();
        stage.join(PlayerProfile { archetype: "acolyte".into(), ..profile(2) }, Uuid::nil(), None, 0, &mut out);
        fog_up_west(&mut stage);
        // 2 stands safe; 1 lies downed in the fog with a long bleed-out.
        put(&mut stage, 2, 4.5, 10.5);
        put(&mut stage, 1, 30.5, 10.5);
        let f = fighter(&mut stage, 1);
        f.hp = 0;
        f.state = Vital::Downed { ticks: 100_000 };
        f.fog = 20;
        let hz = stage.config.tick_hz;
        out.clear();
        run(&mut stage, hz * 5, &mut out);
        assert!(poison_hits(&out).is_empty(), "no damage on the downed");
        assert_eq!(fighter(&mut stage, 1).fog, 25);
        assert!(matches!(fighter(&mut stage, 1).state, Vital::Downed { .. }));
        run(&mut stage, hz * 5, &mut out);
        let f = fighter(&mut stage, 1);
        assert_eq!((f.state, f.fog_dead), (Vital::Dead, true), "dead before the bleed-out ran out");
        assert!(fighter(&mut stage, 2).is_up(), "the safe one is untouched");
        assert_eq!(stage.outcome, Outcome::Running);
    }

    #[test]
    fn a_joiner_gets_the_circle_and_monsters_do_not_mind_it() {
        let mut stage = fog_stage();
        let mut out = Vec::new();
        clock(&mut stage, 720.0);
        stage.tick(&mut out);
        let c = stage.fog.circle.unwrap();
        clock(&mut stage, 800.0);
        let slot = stage.horde_mut().unwrap().spawn(0, 39.5, 19.5).unwrap();
        let hp = stage.horde().unwrap().store().hp[slot as usize];
        out.clear();
        stage.join(profile(3), Uuid::nil(), None, 0, &mut out);
        assert_eq!(
            fogs(&out, 3),
            vec![ServerMsg::Fog {
                begun: false,
                now_ms: 800_000,
                start_ms: 720_000,
                shrink_ms: 150_000,
                x: c.x,
                y: c.y,
                from_radius: c.from_radius,
                to_radius: c.to_radius
            }]
        );
        assert!(c.outside(39.5, 19.5, 800.0));
        run(&mut stage, 60, &mut out);
        let store = stage.horde().unwrap().store();
        assert!(store.is_alive(slot) && store.hp[slot as usize] == hp, "monsters take no toll");
    }
}

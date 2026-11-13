//! Monster skills (COMBAT.md §4.4): the same area × timing × payload
//! effects as players' skills, landing on players.
//!
//! A monster near a player checks its skills about once a second (staggered
//! by slot); a ready skill whose range holds a player and whose chance
//! rolls starts a wind-up: the monster stands still, the party sees the
//! telegraph (`CAST` from the monster's entity id), and the effect lands at
//! the end as [`Hazard`]s. Damage is a saving throw for half against
//! `10 + the monster's save bonus`; ranged skills miss players under a
//! ranged ward (Pneuma).

use super::aim::{norm, Aimed, BODY};
use super::area::Area;
use super::cast::{area_of, count_of, look_of, moved, scaled, ticks16, ticks_of, Live};
use super::fight::damage_type_bits;
use super::{HubEvent, HubInstance, HubPlayer, MapIdx, Outgoing};
use crate::world::combat::rules::saving_throw;
use crate::world::combat::skills::{Act, DamageFx, Effect, EffectDice, MobTarget, MonsterSkill, Payload, SkillBook};
use crate::world::combat::{Condition, DamageType, Dice, Rng};
use crate::world::horde::storage::FLAG_FULL;
use crate::world::horde::Horde;
use crate::world::map::WalkGrid;
use crate::world::protocol::{hit, HitEvent, ServerMsg, Telegraph, MONSTER_ENTITY_BASE};

/// Skills a monster kind may carry.
pub(super) const MAX_MOB_SKILLS: usize = 4;
/// A monster looks at its skills this often (seconds).
const CHECK_SECS: f32 = 1.0;

/// What one monster slot remembers about its skills.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct Mind {
    /// The generation it belongs to (a reused slot starts fresh).
    generation: u8,
    fresh: bool,
    next_check: u32,
    /// In a wind-up until this tick.
    busy_until: u32,
    /// Tick each skill is ready again.
    ready: [u32; MAX_MOB_SKILLS],
}

/// A monster's wind-up.
#[derive(Clone, Copy, Debug)]
pub(super) struct MobCast {
    slot: u16,
    generation: u8,
    /// Index in [`SkillBook::monster`].
    skill: u16,
    land: u32,
    x: f32,
    y: f32,
    dir: (f32, f32),
    zone: u32,
}

/// A monster effect in flight or on the ground.
#[derive(Clone, Debug)]
pub(super) struct Hazard {
    /// Telegraph id of lasting ground (0 = a one-off strike).
    pub id: u32,
    pub map: MapIdx,
    pub next: u32,
    pub until: u32,
    pub pulse: u32,
    pub area: Area,
    dc: i32,
    ranged: bool,
    /// Cast by an undead or a demon.
    evil: bool,
    load: HazardLoad,
}

#[derive(Clone, Copy, Debug)]
enum HazardLoad {
    Damage { dice: Dice, kind: DamageType, rider: Option<(Condition, u16)> },
    Condition { condition: Condition, ticks: u16, save: bool },
}

/// Monster skill indices per kind (catalog order), resolved once.
pub(super) fn kind_skills(horde: &Horde, book: &SkillBook) -> Vec<Vec<u16>> {
    horde
        .catalog()
        .defs()
        .iter()
        .map(|d| {
            let mut v: Vec<u16> = d.skills.iter().filter_map(|id| book.find_monster(id)).collect();
            if v.len() < d.skills.len() {
                tracing::warn!(monster = %d.id, "unknown monster skill ids skipped");
            }
            v.truncate(MAX_MOB_SKILLS);
            v
        })
        .collect()
}

/// Where the monster at `(mx, my)` ends up after the skill's moves, and the
/// areas it will strike (what the telegraphs show).
fn plan<'a>(sk: &'a MonsterSkill, walk: &WalkGrid, mx: f32, my: f32, aim: (f32, f32), dir: (f32, f32)) -> Vec<(Area, &'a Act)> {
    let (mut x, mut y) = (mx, my);
    let mut out = Vec::new();
    for e in &sk.effects {
        match e {
            Effect::Move { mode, distance } => (x, y) = moved(walk, x, y, dir, distance.at(1).max(0.0), *mode),
            Effect::Act(act) => {
                let aimed = Aimed { x: aim.0, y: aim.1, ox: x, oy: y, dir, ally: None, monster: None };
                out.push((area_of(act, 1, sk.range, &aimed, x, y).0, &**act));
            }
            Effect::Passive(_) | Effect::Pending(_) => {}
        }
    }
    out
}

fn telegraphs(planned: &[(Area, &Act)], zone: u32) -> Vec<Telegraph> {
    let mut id = zone;
    planned
        .iter()
        .map(|(area, act)| {
            let t = &act.timing;
            let lasting = t.lasting(1);
            let n = count_of(act, 1);
            let span = if lasting { t.secs.at(1) } else { (n - 1) as f32 * t.every };
            let mut area = *area;
            if act.area.scatter > 0.0
                && let Area::Circle { x, y, r } = area
            {
                area = Area::Circle { x, y, r: r + act.area.scatter };
            }
            let tid = if lasting {
                id += 1;
                id - 1
            } else {
                0
            };
            area.telegraph(tid, look_of(act), (span * 1000.0).round() as u32, 0)
        })
        .collect()
}

/// Turns a finished wind-up into hazards (and moves the monster).
#[allow(clippy::too_many_arguments)]
fn land(c: &MobCast, sk: &MonsterSkill, horde: &mut Horde, walk: &WalkGrid, rng: &mut Rng, now: u32, hz: f32, out: &mut Vec<Hazard>) {
    let store = horde.store();
    let s = c.slot as usize;
    if !store.is_alive(c.slot) || store.generation[s] != c.generation || store.status_of(c.slot).held() {
        return;
    }
    let (mut mx, mut my) = (store.x[s], store.y[s]);
    let block = horde.block(c.slot);
    let dc = 10 + block.save;
    let level = block.level;
    let map = horde.map();
    let evil = horde.catalog().get(store.kind[s]).evil;
    let mut zone = c.zone;
    for e in &sk.effects {
        let act = match e {
            Effect::Move { mode, distance } => {
                (mx, my) = moved(walk, mx, my, c.dir, distance.at(1).max(0.0), *mode);
                horde.place(c.slot, mx, my);
                continue;
            }
            Effect::Act(act) => act,
            Effect::Passive(_) | Effect::Pending(_) => continue,
        };
        let t = &act.timing;
        let lasting = t.lasting(1);
        let load = match &act.payload {
            Payload::Damage(DamageFx { dice: EffectDice::Fixed(d), scale_k, mult, bonus, kind, save, .. }) => HazardLoad::Damage {
                dice: scaled(*d, *scale_k, mult.at(1), bonus.at(1), level),
                kind: *kind,
                rider: save.map(|sv| (sv.condition, ticks16(sv.secs.at(1), hz))),
            },
            Payload::Condition { condition, save, secs, .. } => {
                let ticks = match (*save, lasting) {
                    (true, _) => ticks16(secs.at(1), hz),
                    (false, true) => ticks16(t.pulse * 2.0, hz).max(2),
                    (false, false) => ticks16(1.0, hz),
                };
                HazardLoad::Condition { condition: *condition, ticks, save: *save }
            }
            _ => continue,
        };
        let aimed = Aimed { x: c.x, y: c.y, ox: mx, oy: my, dir: c.dir, ally: None, monster: None };
        let (area, _) = area_of(act, 1, sk.range, &aimed, mx, my);
        let id = if lasting {
            zone += 1;
            zone - 1
        } else {
            0
        };
        for i in 0..count_of(act, 1) {
            let mut area = area;
            if act.area.scatter > 0.0 {
                let a = rng.die(360) as f32 * std::f32::consts::TAU / 360.0;
                let r = act.area.scatter * (rng.die(1000) as f32 / 1000.0).sqrt();
                let (x, y) = area.centre();
                area = area.moved_to(x + a.cos() * r, y + a.sin() * r);
            }
            let at = now + ticks_of(i as f32 * t.every, hz);
            out.push(Hazard {
                id,
                map,
                next: at,
                until: if lasting { at + ticks_of(t.secs.at(1), hz) } else { at },
                pulse: if lasting { ticks_of(t.pulse, hz).max(1) } else { 0 },
                area,
                dc,
                ranged: sk.ranged,
                evil,
                load,
            });
        }
    }
}

/// One action of a hazard on every standing player inside.
fn strike(h: &Hazard, players: &mut [HubPlayer], lives: &[Live], rng: &mut Rng, hz: f32, hits: &mut Vec<HitEvent>) {
    for p in players.iter_mut().filter(|p| p.map == h.map) {
        let (x, y, id) = (p.x, p.y, p.profile.character_id);
        let Some(f) = p.fighter.as_mut() else { continue };
        if !f.is_up() || !h.area.contains(x, y, BODY) {
            continue;
        }
        if h.ranged && lives.iter().any(|l| l.deflects(h.map, x, y)) {
            hits.push(HitEvent { target: id, amount: 0, flags: hit::GUARD });
            continue;
        }
        f.quiet = 0;
        match h.load {
            HazardLoad::Damage { dice, kind, rider } => {
                let bonus = f.save_against(rider.map(|r| r.0));
                let saved = !f.status.saves_fail() && saving_throw(rng, bonus, h.dc, 0);
                let mut amount = dice.roll(rng, false);
                if saved {
                    amount /= 2;
                }
                let taken = f.take_from(amount, kind == DamageType::Physical, h.evil);
                hits.push(HitEvent { target: id, amount: taken as u32, flags: damage_type_bits(kind) });
                if let Some((c, ticks)) = rider
                    && !saved
                {
                    f.afflict(c, ticks);
                }
            }
            HazardLoad::Condition { condition, ticks, save } => {
                let bonus = f.save_against(Some(condition));
                if !save || f.status.saves_fail() || !saving_throw(rng, bonus, h.dc, 0) {
                    f.afflict(condition, ticks);
                }
            }
        }
        f.fall_if_spent(hz);
    }
}

impl HubInstance {
    /// Monster wind-ups, landings and hazards; inside [`HubInstance::fight`]
    /// after the players' attacks, before monster melee.
    pub(super) fn monster_skills(&mut self, out: &mut Outgoing, hits: &mut Vec<HitEvent>) {
        let Some(horde) = self.horde.as_mut() else { return };
        let book = std::sync::Arc::clone(&self.skills);
        let map = horde.map();
        let hz = self.config.tick_hz as f32;
        let now = self.tick;
        if self.kind_skills.len() != horde.catalog().defs().len() {
            self.kind_skills = kind_skills(horde, &book);
        }
        if self.minds.len() != horde.store().capacity() {
            self.minds = vec![Mind::default(); horde.store().capacity()];
        }
        let walk = &self.world.map(map).walk;

        // 1. Wind-ups that end now land.
        let mut i = 0;
        while i < self.mob_casts.len() {
            if self.mob_casts[i].land > now {
                i += 1;
                continue;
            }
            let c = self.mob_casts.swap_remove(i);
            land(&c, book.monster(c.skill), horde, walk, &mut self.dice, now, hz, &mut self.hazards);
        }

        // 2. Hazards act.
        let mut k = 0;
        while k < self.hazards.len() {
            let h = &mut self.hazards[k];
            // Ran its time: the client fades the telegraph on its own.
            if now > h.until {
                self.hazards.swap_remove(k);
                continue;
            }
            if now >= h.next {
                h.next = if h.pulse > 0 { now + h.pulse } else { h.until + 1 };
                let h = h.clone();
                strike(&h, &mut self.players, &self.lives, &mut self.dice, hz, hits);
            }
            k += 1;
        }

        // 3. New wind-ups.
        let ups: Vec<(f32, f32)> = self
            .players
            .iter()
            .filter(|p| p.map == map && p.fighter.as_ref().is_some_and(|f| f.hunted()))
            .map(|p| (p.x, p.y))
            .collect();
        if ups.is_empty() {
            return;
        }
        let mut casts: Vec<ServerMsg> = Vec::new();
        let check = ticks_of(CHECK_SECS, hz).max(1);
        let alive = horde.store().alive().len();
        for a in 0..alive {
            let slot = horde.store().alive()[a];
            let s = slot as usize;
            let store = horde.store();
            let kind = store.kind[s] as usize;
            let skills = &self.kind_skills[kind];
            if skills.is_empty() || store.flags[s] & FLAG_FULL == 0 {
                continue;
            }
            let mind = &mut self.minds[s];
            if mind.generation != store.generation[s] || !mind.fresh {
                *mind = Mind { generation: store.generation[s], fresh: true, next_check: now + 1 + s as u32 % check, ..Mind::default() };
                // Bosses open with their skills ready; the rest wait a cooldown.
                for (j, &sk) in skills.iter().enumerate() {
                    let cd = ticks_of(book.monster(sk).cooldown, hz);
                    mind.ready[j] = if store.flags[s] & crate::world::horde::storage::FLAG_BOSS != 0 { now } else { now + cd / 2 };
                }
                continue;
            }
            if now < mind.next_check || now < mind.busy_until || store.status_of(slot).held() {
                continue;
            }
            mind.next_check = now + check;
            let (mx, my) = (store.x[s], store.y[s]);
            let block = horde.block(slot);
            let hp_frac = (store.hp[s] as f32 / block.hp.max(1) as f32).min(1.0);
            let nearest = ups
                .iter()
                .map(|&(x, y)| ((x - mx).powi(2) + (y - my).powi(2), x, y))
                .min_by(|a, b| a.0.total_cmp(&b.0))
                .map(|(d2, x, y)| (d2.sqrt(), x, y));
            let Some((dist, nx, ny)) = nearest else { continue };
            let mut chosen = None;
            for (j, &sk) in skills.iter().enumerate() {
                let def = book.monster(sk);
                if now < mind.ready[j] || hp_frac > def.hp_below || dist > def.range + BODY {
                    continue;
                }
                if def.chance < 100 && self.dice.die(100) > def.chance {
                    // A miss waits a cooldown's fraction so a crowd does
                    // not roll every second.
                    mind.ready[j] = now + ticks_of(def.cooldown, hz) / 4;
                    continue;
                }
                chosen = Some((j, sk));
                break;
            }
            let Some((j, sk)) = chosen else { continue };
            let def = book.monster(sk);
            let dir = norm(nx - mx, ny - my).unwrap_or((1.0, 0.0));
            let aim = match def.target {
                MobTarget::Caster | MobTarget::Front => (mx, my),
                MobTarget::Nearest => (nx, ny),
                MobTarget::Densest => {
                    let r = def.effects.iter().filter_map(Effect::act).map(|a| a.area.radius.at(1)).fold(1.0f32, f32::max);
                    ups.iter()
                        .filter(|&&(x, y)| (x - mx).powi(2) + (y - my).powi(2) <= (def.range + BODY).powi(2))
                        .max_by_key(|&&(x, y)| ups.iter().filter(|&&(ox, oy)| (ox - x).powi(2) + (oy - y).powi(2) <= r * r).count())
                        .copied()
                        .unwrap_or((nx, ny))
                }
            };
            let cast = ticks_of(def.cast, hz);
            mind.ready[j] = now + ticks_of(def.cooldown, hz);
            mind.busy_until = now + cast + 1;
            let zone = self.next_zone;
            let planned = plan(def, walk, mx, my, aim, dir);
            let lasting = planned.iter().filter(|(_, a)| a.timing.lasting(1)).count() as u32;
            self.next_zone = self.next_zone.wrapping_add(lasting).max(1);
            casts.push(ServerMsg::Cast {
                caster: MONSTER_ENTITY_BASE + slot as u32,
                skill: def.id.clone(),
                x: aim.0,
                y: aim.1,
                cast_ms: (def.cast * 1000.0).round() as u32,
                cooldown_ms: 0,
                areas: telegraphs(&planned, zone),
                level: 1,
            });
            self.mob_casts.push(MobCast { slot, generation: store.generation[s], skill: sk, land: now + cast, x: aim.0, y: aim.1, dir, zone });
            // It stands still and holds its melee through the wind-up.
            let store = horde.store_mut();
            store.afflict(slot, Condition::Rooted, (cast + 1).min(u16::MAX as u32) as u16);
            store.cooldown[s] = store.cooldown[s].max((cast + 2).min(u16::MAX as u32) as u16);
        }
        if !casts.is_empty() {
            for p in self.players.iter().filter(|p| p.map == map) {
                for m in &casts {
                    out.push((p.profile.user_id, HubEvent::Control(m.clone())));
                }
            }
        }
    }

    /// Ends monster ground inside `area` (Ganbantein); returns the
    /// telegraph ids to end on the clients.
    pub(super) fn dispel_hazards(&mut self, map: MapIdx, area: &Area) -> Vec<u32> {
        let mut ids = Vec::new();
        self.hazards.retain(|h| {
            let (x, y) = h.area.centre();
            let hit = h.map == map && h.pulse > 0 && area.contains(x, y, 0.0);
            if hit && h.id != 0 {
                ids.push(h.id);
            }
            !hit
        });
        ids
    }
}

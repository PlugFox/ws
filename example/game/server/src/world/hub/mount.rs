//! Riding (COMBAT.md §3.2, Cavalry): a Crusader or Paladin with Peco Peco
//! Ride mounts and dismounts anywhere (`MOUNT_REQ`, through the session's
//! loadout, so it is kept between sessions). The instance only tells the
//! map (`PLAYER_MOUNT`, drawn with the mounted body) and walks a rider in
//! town at the mount's pace; on a stage the fighter's riding passives
//! (pace, attack interval, spear bonuses) come with `cast::equip`.

use super::{HubEvent, HubInstance, HubPlayer, Outgoing};
use crate::world::combat::{Class, Loadout, SkillBook, Weapon};
use crate::world::protocol::{mount, ServerMsg};

impl HubPlayer {
    /// What riding adds to the walking pace (a fraction; 0 on foot): the
    /// riding passives' speed, with the weapon in hand.
    pub(super) fn ride_bonus(&self, book: &SkillBook) -> f32 {
        let loadout = &self.profile.loadout;
        if !book.rides(loadout) {
            return 0.0;
        }
        let class = Class::from_id(&self.profile.archetype).unwrap_or(Class::Swordsman);
        let kind = Weapon::held(class, self.weapon_id()).kind;
        let on_foot = Loadout { mounted: false, ..loadout.clone() };
        (book.passives(class, loadout, kind).speed - book.passives(class, &on_foot, kind).speed).max(0.0)
    }

    /// `player_mount` of this player (`ride` as [`Self::ride_bonus`] gave it).
    fn mount_msg(&self, book: &SkillBook) -> ServerMsg {
        let riding = book.rides(&self.profile.loadout);
        ServerMsg::PlayerMount {
            entity_id: self.profile.character_id,
            mount: if riding { mount::PECO } else { mount::NONE },
            speed: (self.ride * 100.0).round().clamp(0.0, 255.0) as u8,
        }
    }
}

impl HubInstance {
    /// After `map_enter`: everyone riding on the player's map, the player
    /// included.
    pub(super) fn announce_mounts(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        for o in self.players.iter().filter(|o| o.map == p.map && self.skills.rides(&o.profile.loadout)) {
            out.push((p.profile.user_id, HubEvent::Control(o.mount_msg(&self.skills))));
        }
    }

    /// After `player_appeared`: the newcomer's mount to the others there.
    pub(super) fn announce_rider(&self, idx: usize, out: &mut Outgoing) {
        let p = &self.players[idx];
        if !self.skills.rides(&p.profile.loadout) {
            return;
        }
        let msg = p.mount_msg(&self.skills);
        for o in self.players.iter().filter(|o| o.map == p.map && o.profile.user_id != p.profile.user_id) {
            out.push((o.profile.user_id, HubEvent::Control(msg.clone())));
        }
    }

    /// After a loadout change: a rider who got on or off (`rode`: before)
    /// walks at the new pace, and the whole map sees it.
    pub(super) fn remount(&mut self, idx: usize, rode: bool, out: &mut Outgoing) {
        let ride = self.players[idx].ride_bonus(&self.skills);
        self.players[idx].ride = ride;
        let p = &self.players[idx];
        if self.skills.rides(&p.profile.loadout) == rode {
            return;
        }
        let msg = p.mount_msg(&self.skills);
        for o in self.players.iter().filter(|o| o.map == p.map) {
            out.push((o.profile.user_id, HubEvent::Control(msg.clone())));
        }
    }
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::world::combat::WeaponKind;

    /// Peco Peco Ride and its riding pace, Spear Mastery with a riding
    /// bonus, Brandish Spear (riding and a spear).
    pub(in crate::world::hub) const BOOK: &str = r#"{"skills":[
        {"id":"peco_ride","name":"Peco Peco Ride","class":"swordsman","tier":2,"max_level":1,"min_level":30,
         "target":"self","effects":[{"type":"mount"},{"type":"buff","riding":true,"speed":0.25,"haste":-0.25}]},
        {"id":"spear_mastery","name":"Spear Mastery","class":"swordsman","tier":2,"max_level":10,"min_level":30,"passive":true,
         "effects":[{"type":"buff","weapon":"spear","damage_flat":[0,0.5]},{"type":"buff","weapon":"spear","riding":true,"damage_flat":[0,0.5]}]},
        {"id":"brandish","name":"Brandish Spear","class":"swordsman","tier":2,"max_level":10,"min_level":30,"riding":true,"weapon":"spear",
         "target":"front","range":3.5,"effects":[{"type":"damage","dice":"weapon","shape":"arc","arc":120,"radius":3.0}]},
        {"id":"piercing_arrow","name":"Piercing Arrow","class":"archer","tier":2,"max_level":10,"min_level":30,
         "target":"line","range":14,"effects":[{"type":"damage","shape":"line","width":0.8,"dice":"1d4","projectile":30}]}
    ]}"#;

    pub(in crate::world::hub) fn rider(mounted: bool) -> Loadout {
        let mut l = Loadout { mounted, ..Loadout::default() };
        l.skills.insert("peco_ride".into(), 1);
        l.skills.insert("spear_mastery".into(), 10);
        l.skills.insert("brandish".into(), 1);
        l
    }

    #[test]
    fn riding_needs_the_skill_and_its_passives_count_only_mounted() {
        let b = SkillBook::from_json(BOOK).unwrap();
        assert!(b.rides(&rider(true)));
        assert!(!b.rides(&rider(false)), "learned but on foot");
        let mut no_skill = rider(true);
        no_skill.skills.remove("peco_ride");
        assert!(!b.rides(&no_skill), "mounted without the skill is on foot");
        b.sanitize(Class::Swordsman, &mut no_skill);
        assert!(!no_skill.mounted, "sanitize drops a mount nobody can ride");

        let (spear, sword) = (WeaponKind::Spear, WeaponKind::Sword);
        let on = b.passives(Class::Swordsman, &rider(true), spear);
        let off = b.passives(Class::Swordsman, &rider(false), spear);
        assert_eq!((on.damage_flat, off.damage_flat), (10, 5), "Spear Mastery 10: +5, +10 riding");
        assert!((on.speed - 0.25).abs() < 1e-6 && off.speed == 0.0, "the mount's pace only mounted");
        assert!((on.interval() - 1.25).abs() < 1e-6, "riding slows the swing");
        assert_eq!(b.passives(Class::Swordsman, &rider(true), sword).damage_flat, 0, "no spear, no mastery");
        assert_eq!(b.passives(Class::Archer, &rider(true), spear), crate::world::combat::Mods::default(), "another class's skills");

        let (_, brandish) = b.find("brandish").unwrap();
        assert!(brandish.usable_with(true, spear));
        assert!(!brandish.usable_with(false, spear), "on foot");
        assert!(!brandish.usable_with(true, sword), "without a spear");
        let (_, ride) = b.find("peco_ride").unwrap();
        assert!(ride.mounts() && ride.ready() && !brandish.mounts());
        // The riding pace of an active skill is a passive, not a cast.
        assert!(ride.effects.iter().all(|e| e.act().is_none()));
    }

    #[test]
    fn unknown_weapon_families_are_rejected() {
        let bad = r#"{"skills":[{"id":"x","name":"X","class":"swordsman","weapon":"lance","effects":[{"type":"mount"}]}]}"#;
        assert!(SkillBook::from_json(bad).is_err());
    }
}

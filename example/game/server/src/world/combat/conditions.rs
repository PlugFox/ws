//! Conditions (COMBAT.md §2.5): what a failed save leaves on a monster.
//!
//! A monster carries one timer per condition (ticks left) and the bit set
//! of the active ones ([`Status`]), which is also what clients see (a
//! varint on the wire: the first seven fit one byte).

use super::class::Save;
use super::rules::Element;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Condition {
    /// Movement −50%.
    Slowed,
    /// Cannot act or move; attacks against it have advantage; physical
    /// hits shatter the ice for +1d6 per k; counts as water.
    Frozen,
    /// Cannot act or move; its saves fail.
    Stunned,
    /// Disadvantage on its attacks.
    Blinded,
    /// Cannot move, can attack.
    Rooted,
    /// Frozen, plus resistance to all damage (not water, not shattered).
    Petrified,
    /// 1d4 fire per k each second (the k of whoever lit it last).
    Burning,
    /// Disadvantage on its attacks; 1d4 poison each second.
    Poisoned,
    /// −2 AC (standing in a swamp).
    Mired,
    /// Hunts and strikes whoever taunted it (Provoke), not the nearest;
    /// −2 AC and disadvantage on attacks against anyone else.
    Taunted,
    /// Terrified (a ghost's wail): it flees from what scared it and can
    /// neither attack nor cast (players; COMBAT.md, Payon Caves).
    Feared,
}

/// Conditions a monster can carry (timers per slot).
pub const CONDITIONS: usize = 11;

impl Condition {
    pub const ALL: [Condition; CONDITIONS] = [
        Condition::Slowed,
        Condition::Frozen,
        Condition::Stunned,
        Condition::Blinded,
        Condition::Rooted,
        Condition::Petrified,
        Condition::Burning,
        Condition::Poisoned,
        Condition::Mired,
        Condition::Taunted,
        Condition::Feared,
    ];

    /// The skill table's names.
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "slowed" => Condition::Slowed,
            "frozen" => Condition::Frozen,
            "stunned" => Condition::Stunned,
            "blinded" => Condition::Blinded,
            "rooted" => Condition::Rooted,
            "petrified" => Condition::Petrified,
            "burning" => Condition::Burning,
            "poisoned" => Condition::Poisoned,
            "mired" => Condition::Mired,
            "taunted" => Condition::Taunted,
            "feared" => Condition::Feared,
            _ => return None,
        })
    }

    pub const fn bit(self) -> u16 {
        1 << self as u16
    }

    /// The save that resists it: dodging (Reflex), enduring (Body) or
    /// keeping one's head (Mind).
    pub const fn save(self) -> Save {
        match self {
            Condition::Slowed | Condition::Frozen | Condition::Rooted | Condition::Mired => Save::Reflex,
            Condition::Stunned | Condition::Petrified | Condition::Burning | Condition::Poisoned => Save::Body,
            Condition::Blinded | Condition::Taunted | Condition::Feared => Save::Mind,
        }
    }
}

/// Bit set of active conditions (`Condition::bit`), as sent to clients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Status(pub u16);

impl Status {
    pub const fn has(self, c: Condition) -> bool {
        self.0 & c.bit() != 0
    }

    const fn any(self, mask: u16) -> bool {
        self.0 & mask != 0
    }

    const HELD: u16 = Condition::Frozen.bit() | Condition::Stunned.bit() | Condition::Petrified.bit();

    /// Frozen, stunned or petrified: no attacks, no steps.
    pub const fn held(self) -> bool {
        self.any(Self::HELD)
    }

    /// Held, or too terrified to fight: no attacks, no casts.
    pub const fn helpless(self) -> bool {
        self.any(Self::HELD | Condition::Feared.bit())
    }

    pub const fn can_move(self) -> bool {
        !self.any(Self::HELD | Condition::Rooted.bit())
    }

    /// Multiplier of its walking speed.
    pub const fn speed(self) -> f32 {
        if self.has(Condition::Slowed) {
            0.5
        } else {
            1.0
        }
    }

    /// Advantage of attack rolls against it (5e: helpless targets).
    pub const fn advantage_against(self) -> i8 {
        if self.held() {
            1
        } else {
            0
        }
    }

    /// Advantage of its own attacks.
    pub const fn advantage(self) -> i8 {
        if self.any(Condition::Blinded.bit() | Condition::Poisoned.bit()) {
            -1
        } else {
            0
        }
    }

    /// Change of its armour class (swamp, provoked).
    pub const fn ac(self) -> i32 {
        let mut ac = 0;
        if self.has(Condition::Mired) {
            ac -= 2;
        }
        if self.has(Condition::Taunted) {
            ac -= 2;
        }
        ac
    }

    /// Takes damage every second (burning, poisoned).
    pub const fn ailing(self) -> bool {
        self.any(Condition::Burning.bit() | Condition::Poisoned.bit())
    }

    /// Stunned: saving throws fail.
    pub const fn saves_fail(self) -> bool {
        self.has(Condition::Stunned)
    }

    /// Element for affinities: ice makes it water.
    pub const fn element(self, base: Element) -> Element {
        if self.has(Condition::Frozen) {
            Element::Water
        } else {
            base
        }
    }

    /// Petrified: resistance to all damage.
    pub const fn resists_all(self) -> bool {
        self.has(Condition::Petrified)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_rules() {
        let s = |cs: &[Condition]| Status(cs.iter().fold(0, |b, c| b | c.bit()));
        assert!(s(&[Condition::Frozen]).held() && !s(&[Condition::Frozen]).can_move());
        assert!(!s(&[Condition::Rooted]).held() && !s(&[Condition::Rooted]).can_move());
        assert_eq!(s(&[Condition::Slowed]).speed(), 0.5);
        assert_eq!(s(&[Condition::Stunned]).advantage_against(), 1);
        assert_eq!(s(&[Condition::Blinded]).advantage(), -1);
        assert_eq!(s(&[Condition::Frozen]).element(Element::Fire), Element::Water);
        assert_eq!(s(&[Condition::Petrified]).element(Element::Fire), Element::Fire);
        assert!(s(&[Condition::Stunned]).saves_fail());
        assert_eq!(s(&[Condition::Poisoned]).advantage(), -1);
        assert_eq!(s(&[Condition::Mired]).ac(), -2);
        assert_eq!(s(&[Condition::Mired, Condition::Taunted]).ac(), -4);
        assert_eq!(Condition::Poisoned.save(), Save::Body);
        assert_eq!(Condition::Slowed.save(), Save::Reflex);
        assert!(s(&[Condition::Burning]).ailing() && !s(&[Condition::Slowed]).ailing());
        assert_eq!(Condition::from_id("petrified"), Some(Condition::Petrified));
        assert_eq!(Condition::from_id("bleeding"), None);
        assert!(Condition::ALL.iter().enumerate().all(|(i, c)| *c as usize == i));
        let feared = s(&[Condition::Feared]);
        assert!(feared.helpless() && !feared.held() && feared.can_move());
        assert!(s(&[Condition::Stunned]).helpless() && !s(&[Condition::Slowed]).helpless());
        assert_eq!((Condition::from_id("feared"), Condition::Feared.save()), (Some(Condition::Feared), Save::Mind));
    }
}

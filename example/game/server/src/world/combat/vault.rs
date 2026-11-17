//! The account storage (`session::storage`): stacks and zeny every
//! character of an account shares. Pure rules here; the session moves
//! things in one database transaction with the character's bag.

use super::items::{parse_key, Bag, ItemError, Stack, MAX_STACK, MAX_ZENY};

/// Stacks the storage holds.
pub const VAULT_STACKS: usize = 600;
/// Most zeny the storage holds.
pub const VAULT_ZENY: u64 = 10_000_000_000;
/// Most stacks one move names.
pub const MAX_MOVE: usize = 600;

/// What an account keeps in storage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Vault {
    pub zeny: u64,
    pub items: Vec<Stack>,
}

/// Which way a move goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dir {
    /// Bag → storage.
    Put,
    /// Storage → bag.
    Take,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultError {
    /// Nothing named, a zero count, too many stacks in one move.
    Empty,
    /// The bag lacks that many (`Put`), or the item is unknown.
    NotInBag,
    /// The storage lacks that many (`Take`).
    NotStored,
    /// No room in the storage.
    VaultFull,
    /// No room in the bag.
    BagFull,
    NoZeny,
    /// Over the storage's zeny cap or the bag's.
    TooMuchZeny,
}

impl VaultError {
    pub const fn code(self) -> &'static str {
        match self {
            VaultError::Empty => "storage_empty_move",
            VaultError::NotInBag => "not_in_bag",
            VaultError::NotStored => "not_stored",
            VaultError::VaultFull => "storage_full",
            VaultError::BagFull => "bag_full",
            VaultError::NoZeny => "not_enough_zeny",
            VaultError::TooMuchZeny => "too_much_zeny",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            VaultError::Empty => "Nothing to move.",
            VaultError::NotInBag => "You do not have that.",
            VaultError::NotStored => "That is not in storage.",
            VaultError::VaultFull => "The storage is full.",
            VaultError::BagFull => "Your bag is full.",
            VaultError::NoZeny => "Not enough zeny.",
            VaultError::TooMuchZeny => "That much zeny does not fit there.",
        }
    }
}

impl Vault {
    /// From the stored form. Copies of one instance merge; stacks of
    /// items the catalogue no longer knows are kept as they are (never
    /// lost: they come back if the item does), just not movable.
    pub fn load(items: Vec<Stack>, zeny: i64) -> Vault {
        let mut v = Vault { zeny: zeny.max(0) as u64, items: Vec::with_capacity(items.len()) };
        for s in items.into_iter().filter(|s| s.count > 0) {
            match v.items.iter().position(|o| o.is(&s.id, &s.attrs)) {
                Some(i) => v.items[i].count = v.items[i].count.saturating_add(s.count),
                None => v.items.push(s),
            }
        }
        v
    }

    /// How many of the instance `key` are stored.
    pub fn count(&self, key: &str) -> u32 {
        let Some((e, attrs)) = parse_key(key) else { return 0 };
        self.items.iter().find(|s| s.is(e.id, &attrs)).map_or(0, |s| s.count)
    }

    fn put(&mut self, key: &str, n: u32) -> Result<(), VaultError> {
        let (e, attrs) = parse_key(key).ok_or(VaultError::NotInBag)?;
        match self.items.iter().position(|s| s.is(e.id, &attrs)) {
            Some(i) => {
                let sum = self.items[i].count.checked_add(n).filter(|&c| c <= MAX_STACK).ok_or(VaultError::VaultFull)?;
                self.items[i].count = sum;
            }
            None if self.items.len() >= VAULT_STACKS || n > MAX_STACK => return Err(VaultError::VaultFull),
            None => self.items.push(Stack { id: e.id.into(), count: n, attrs }),
        }
        Ok(())
    }

    fn take(&mut self, key: &str, n: u32) -> Result<(), VaultError> {
        let (e, attrs) = parse_key(key).ok_or(VaultError::NotStored)?;
        let i = self.items.iter().position(|s| s.is(e.id, &attrs)).ok_or(VaultError::NotStored)?;
        if self.items[i].count < n {
            return Err(VaultError::NotStored);
        }
        self.items[i].count -= n;
        if self.items[i].count == 0 {
            self.items.remove(i);
        }
        Ok(())
    }
}

/// Moves `items` (instance keys, counts) and `zeny` between `bag` and
/// `vault`, all or nothing: on an error both are as they were.
pub fn transfer(bag: &mut Bag, vault: &mut Vault, dir: Dir, items: &[(String, u32)], zeny: u64) -> Result<(), VaultError> {
    if (items.is_empty() && zeny == 0) || items.len() > MAX_MOVE || items.iter().any(|(_, n)| *n == 0) {
        return Err(VaultError::Empty);
    }
    let (mut b, mut v) = (bag.clone(), vault.clone());
    for (key, n) in items {
        match dir {
            Dir::Put => {
                b.take(key, *n).map_err(|_| VaultError::NotInBag)?;
                v.put(key, *n)?;
            }
            Dir::Take => {
                v.take(key, *n)?;
                b.add(key, *n).map_err(|e| if e == ItemError::BagFull { VaultError::BagFull } else { VaultError::NotStored })?;
            }
        }
    }
    let (from, to, cap) = match dir {
        Dir::Put => (&mut b.zeny, &mut v.zeny, VAULT_ZENY),
        Dir::Take => (&mut v.zeny, &mut b.zeny, MAX_ZENY),
    };
    *from = from.checked_sub(zeny).ok_or(VaultError::NoZeny)?;
    *to = to.checked_add(zeny).filter(|&z| z <= cap).ok_or(VaultError::TooMuchZeny)?;
    *bag = b;
    *vault = v;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::items::MAX_STACKS;

    fn bag(zeny: u64, items: &[(&str, u32)]) -> Bag {
        let mut b = Bag { zeny, ..Bag::default() };
        for (k, n) in items {
            b.add(k, *n).unwrap();
        }
        b
    }

    fn keys(v: &[(&str, u32)]) -> Vec<(String, u32)> {
        v.iter().map(|(k, n)| (k.to_string(), *n)).collect()
    }

    #[test]
    fn a_move_is_all_or_nothing() {
        let mut b = bag(500, &[("red_potion", 10), ("jellopy", 3)]);
        let mut v = Vault::default();
        assert_eq!(transfer(&mut b, &mut v, Dir::Put, &keys(&[("red_potion", 4), ("jellopy", 3)]), 200), Ok(()));
        assert_eq!((b.count("red_potion"), b.count("jellopy"), b.zeny), (6, 0, 300));
        assert_eq!((v.count("red_potion"), v.count("jellopy"), v.zeny), (4, 3, 200));
        // The second stack is short: nothing moves, the first included.
        let (b0, v0) = (b.clone(), v.clone());
        assert_eq!(transfer(&mut b, &mut v, Dir::Take, &keys(&[("red_potion", 4), ("jellopy", 9)]), 0), Err(VaultError::NotStored));
        assert_eq!((&b, &v), (&b0, &v0));
        assert_eq!(transfer(&mut b, &mut v, Dir::Take, &[], 201), Err(VaultError::NoZeny));
        assert_eq!(transfer(&mut b, &mut v, Dir::Put, &[], 0), Err(VaultError::Empty));
        assert_eq!(transfer(&mut b, &mut v, Dir::Put, &keys(&[("red_potion", 0)]), 0), Err(VaultError::Empty));
        assert_eq!((&b, &v), (&b0, &v0));
        assert_eq!(transfer(&mut b, &mut v, Dir::Take, &keys(&[("red_potion", 4)]), 200), Ok(()));
        assert_eq!((b.count("red_potion"), b.zeny, v.count("red_potion"), v.zeny), (10, 500, 0, 0));
    }

    #[test]
    fn refined_and_carded_copies_keep_their_key() {
        let mut b = bag(0, &[("sword+7", 1), ("sword", 2)]);
        let mut v = Vault::default();
        transfer(&mut b, &mut v, Dir::Put, &keys(&[("sword+7", 1), ("sword", 1)]), 0).unwrap();
        assert_eq!((v.count("sword+7"), v.count("sword"), v.items.len()), (1, 1, 2));
        transfer(&mut b, &mut v, Dir::Take, &keys(&[("sword+7", 1)]), 0).unwrap();
        assert_eq!(b.count("sword+7"), 1);
    }

    #[test]
    fn caps_hold_on_both_sides() {
        // A full bag takes nothing new out of storage.
        let mut b = Bag::default();
        let mut v = Vault::default();
        v.put("jellopy", 5).unwrap();
        for i in 0..MAX_STACKS {
            b.items.push(Stack { id: format!("x{i}"), count: 1, ..Stack::default() });
        }
        assert_eq!(transfer(&mut b, &mut v, Dir::Take, &keys(&[("jellopy", 1)]), 0), Err(VaultError::BagFull));
        assert_eq!(v.count("jellopy"), 5);
        // Zeny caps.
        let mut b = bag(MAX_ZENY, &[]);
        let mut v = Vault { zeny: 10, ..Vault::default() };
        assert_eq!(transfer(&mut b, &mut v, Dir::Take, &[], 1), Err(VaultError::TooMuchZeny));
        let mut v = Vault { zeny: VAULT_ZENY, ..Vault::default() };
        assert_eq!(transfer(&mut b, &mut v, Dir::Put, &[], 1), Err(VaultError::TooMuchZeny));
        // A stack's cap.
        let mut b = bag(0, &[("jellopy", 10)]);
        let mut v = Vault::default();
        v.put("jellopy", MAX_STACK - 5).unwrap();
        assert_eq!(transfer(&mut b, &mut v, Dir::Put, &keys(&[("jellopy", 10)]), 0), Err(VaultError::VaultFull));
        assert_eq!(b.count("jellopy"), 10);
    }

    #[test]
    fn unknown_stored_items_are_kept() {
        let v = Vault::load(vec![Stack { id: "gone_item".into(), count: 3, ..Stack::default() }, Stack { id: "jellopy".into(), count: 2, ..Stack::default() }, Stack { id: "jellopy".into(), count: 1, ..Stack::default() }], 5);
        assert_eq!(v.items.len(), 2);
        assert_eq!(v.count("jellopy"), 3);
        assert_eq!(v.items[0].id, "gone_item");
    }
}

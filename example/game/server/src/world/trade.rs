//! Player-to-player trade (COMBAT.md §2.6.2): what each side puts on the
//! table, the exchange itself as a pure function of the two bags, and the
//! hand-off of a confirmed deal to whoever owns the belongings.
//!
//! The hub (`hub::trade`) runs the request → offer → lock → confirm state
//! machine against its copy of each bag. The belongings themselves live
//! with the session's character, so a deal both sides confirmed goes to a
//! [`TradeVault`]: it locks both characters, re-runs [`exchange`] on the
//! live bags and stores both in one database transaction
//! (`session::trade::SessionVault`). Nothing moves unless all of it does.

use super::combat::items::{self, Bag, ItemError, MAX_ZENY};
use super::wire::{PutWire, Reader};
use std::future::Future;
use std::pin::Pin;

/// Most different items one side may offer (RO's ten).
pub const MAX_OFFER_ITEMS: usize = 10;

/// One side of the table: stacks from the bag (never what is worn) and zeny.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Offer {
    pub items: Vec<(String, u32)>,
    pub zeny: u64,
}

/// Why a trade step or the exchange was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TradeError {
    /// The item is not (or no longer) in the bag in that number.
    NotOwned,
    /// More zeny offered than owned.
    NoZeny,
    /// Too many different items on one side.
    TooManyItems,
    /// A receiving bag has no room (stacks or slots).
    BagFull,
    /// A receiver would go over the zeny cap.
    ZenyLimit,
    /// A side is gone (logged out, another character, another session).
    Gone,
    /// Storing the result failed: nothing changed.
    Storage,
    /// No vault on this server (tests without a database).
    Unavailable,
}

impl TradeError {
    pub const fn code(self) -> &'static str {
        match self {
            TradeError::NotOwned => "trade_not_owned",
            TradeError::NoZeny => "not_enough_zeny",
            TradeError::TooManyItems => "trade_too_many",
            TradeError::BagFull => "trade_bag_full",
            TradeError::ZenyLimit => "trade_zeny_limit",
            TradeError::Gone => "trade_partner_gone",
            TradeError::Storage => "trade_failed",
            TradeError::Unavailable => "trade_unavailable",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            TradeError::NotOwned => "You do not have that many.",
            TradeError::NoZeny => "Not enough zeny.",
            TradeError::TooManyItems => "At most ten different items per trade.",
            TradeError::BagFull => "A bag cannot hold everything offered.",
            TradeError::ZenyLimit => "That would go over the zeny limit.",
            TradeError::Gone => "Your partner is no longer here.",
            TradeError::Storage => "The trade could not be stored; nothing changed.",
            TradeError::Unavailable => "Trading is not available on this server.",
        }
    }
}

impl From<ItemError> for TradeError {
    fn from(e: ItemError) -> Self {
        match e {
            ItemError::BagFull => TradeError::BagFull,
            ItemError::NoZeny => TradeError::NoZeny,
            _ => TradeError::NotOwned,
        }
    }
}

impl Offer {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.zeny == 0
    }

    /// How many of `id` are on the table.
    pub fn count(&self, id: &str) -> u32 {
        self.items.iter().find(|(i, _)| i == id).map_or(0, |(_, n)| *n)
    }

    /// Puts `count` of `id` on the table (replacing what was there; 0
    /// takes it off), checked against `bag`'s stacks: worn items are not
    /// in them, so they cannot be offered.
    pub fn set_item(&mut self, bag: &Bag, id: &str, count: u32) -> Result<(), TradeError> {
        let at = self.items.iter().position(|(i, _)| i == id);
        if count == 0 {
            if let Some(i) = at {
                self.items.remove(i);
            }
            return Ok(());
        }
        if items::parse_key(id).is_none() || bag.count(id) < count {
            return Err(TradeError::NotOwned);
        }
        match at {
            Some(i) => self.items[i].1 = count,
            None if self.items.len() >= MAX_OFFER_ITEMS => return Err(TradeError::TooManyItems),
            None => self.items.push((id.into(), count)),
        }
        Ok(())
    }

    /// Puts `zeny` on the table (0 takes it off).
    pub fn set_zeny(&mut self, bag: &Bag, zeny: u64) -> Result<(), TradeError> {
        if zeny > bag.zeny || zeny > MAX_ZENY {
            return Err(TradeError::NoZeny);
        }
        self.zeny = zeny;
        Ok(())
    }

    /// Whether `bag` still holds all of it.
    pub fn covered_by(&self, bag: &Bag) -> bool {
        self.zeny <= bag.zeny && self.items.iter().all(|(id, n)| *n > 0 && bag.count(id) >= *n)
    }

    /// Shrinks the offer to what `bag` still holds; returns whether it
    /// changed.
    pub fn clamp_to(&mut self, bag: &Bag) -> bool {
        let before = self.clone();
        self.zeny = self.zeny.min(bag.zeny);
        for (id, n) in &mut self.items {
            *n = (*n).min(bag.count(id));
        }
        self.items.retain(|(_, n)| *n > 0);
        *self != before
    }
}

/// The exchange: `a` gives `give_a` to `b`, `b` gives `give_b` to `a`.
/// Returns both new bags, or why not (and then neither changes). Offers
/// come out of the stacks first, so a side can receive into the room it
/// frees.
pub fn exchange(a: &Bag, b: &Bag, give_a: &Offer, give_b: &Offer) -> Result<(Bag, Bag), TradeError> {
    if give_a.items.len() > MAX_OFFER_ITEMS || give_b.items.len() > MAX_OFFER_ITEMS {
        return Err(TradeError::TooManyItems);
    }
    let (mut na, mut nb) = (a.clone(), b.clone());
    for (bag, give) in [(&mut na, give_a), (&mut nb, give_b)] {
        if give.zeny > bag.zeny {
            return Err(TradeError::NoZeny);
        }
        bag.zeny -= give.zeny;
        for (id, n) in &give.items {
            bag.take(id, *n).map_err(|_| TradeError::NotOwned)?;
        }
    }
    for (bag, get) in [(&mut na, give_b), (&mut nb, give_a)] {
        let zeny = bag.zeny.checked_add(get.zeny).filter(|z| *z <= MAX_ZENY).ok_or(TradeError::ZenyLimit)?;
        bag.zeny = zeny;
        for (id, n) in &get.items {
            bag.add(id, *n)?;
        }
    }
    Ok((na, nb))
}

/// A trade step from the client (`ClientMsg::Trade`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TradeOp {
    /// Ask the player `entity_id` (nearby, in town) to trade.
    Request { entity_id: u32 },
    /// Answer the pending request to the sender.
    Accept,
    Decline,
    /// Close the open trade, or withdraw / turn down a request.
    Cancel,
    /// Freeze the own offer ("OK"); any change unlocks both sides.
    Lock,
    /// Both locked: agree to the exchange ("Trade").
    Confirm,
    /// Put this much zeny on the table (0 takes it off).
    Zeny(u64),
    /// Put `count` of `item` on the table (0 takes it off).
    Item { item: String, count: u32 },
}

impl TradeOp {
    /// Wire form after the kind byte (`protocol.rs`, 0x5B TRADE).
    pub fn put(&self, b: &mut impl PutWire) {
        match self {
            TradeOp::Request { entity_id } => {
                b.put_u8(0);
                b.put_var((*entity_id).into());
            }
            TradeOp::Accept => b.put_u8(1),
            TradeOp::Decline => b.put_u8(2),
            TradeOp::Cancel => b.put_u8(3),
            TradeOp::Lock => b.put_u8(4),
            TradeOp::Confirm => b.put_u8(5),
            TradeOp::Zeny(z) => {
                b.put_u8(6);
                b.put_var(*z);
            }
            TradeOp::Item { item, count } => {
                b.put_u8(7);
                b.put_str(item);
                b.put_var((*count).into());
            }
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        Some(match r.u8()? {
            0 => TradeOp::Request { entity_id: r.var32()? },
            1 => TradeOp::Accept,
            2 => TradeOp::Decline,
            3 => TradeOp::Cancel,
            4 => TradeOp::Lock,
            5 => TradeOp::Confirm,
            6 => TradeOp::Zeny(r.var()?),
            7 => TradeOp::Item { item: r.str()?, count: r.var32()? },
            _ => return None,
        })
    }
}

impl Offer {
    /// Wire form: var zeny, var n, n × {str item, var count}.
    pub fn put(&self, b: &mut impl PutWire) {
        b.put_var(self.zeny);
        b.put_var(self.items.len() as u64);
        for (id, n) in &self.items {
            b.put_str(id);
            b.put_var((*n).into());
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        let zeny = r.var()?;
        let n = r.var()?;
        if n > MAX_OFFER_ITEMS as u64 {
            return None;
        }
        let items = (0..n).map(|_| Some((r.str()?, r.var32()?))).collect::<Option<_>>()?;
        Some(Offer { items, zeny })
    }
}

/// One side of a confirmed deal.
#[derive(Clone, Debug, PartialEq)]
pub struct Party {
    pub user_id: u32,
    pub character_id: u32,
    /// What this side gives.
    pub offer: Offer,
}

/// A trade both sides locked and confirmed, to be settled by a
/// [`TradeVault`]. `id` is the hub's trade id (unique per instance).
#[derive(Clone, Debug, PartialEq)]
pub struct Deal {
    pub id: u32,
    pub sides: [Party; 2],
}

pub type Settlement = Result<[Bag; 2], TradeError>;

/// Applies a deal to the owners of the belongings, atomically: either
/// both bags change (in memory and in storage) or neither does. Returns
/// the new bags in `sides` order.
pub trait TradeVault: Send + Sync + 'static {
    fn settle(&self, deal: Deal) -> Pin<Box<dyn Future<Output = Settlement> + Send + 'static>>;
}

/// Most lines on each side (buy, sell) of one merchant order.
pub const MAX_ORDER_LINES: usize = 20;
/// Most of one item on one order line (the shop window's cap).
pub const MAX_ORDER_COUNT: u32 = 999;

/// A merchant order (`ClientMsg::ShopDeal`): the shop window's cart, buys
/// and sells together, made all at once or not at all. Selling goes
/// first, so the loot sold pays for what is bought and frees bag room.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ShopOrder {
    /// Catalogue ids from the merchant's stock, with counts.
    pub buy: Vec<(String, u32)>,
    /// Instance keys from the bag (never what is worn), with counts.
    pub sell: Vec<(String, u32)>,
}

/// What an order cost and brought in (`ServerMsg::ShopDone`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Receipt {
    pub spent: u64,
    pub earned: u64,
}

/// Why an order is refused before it reaches the bag.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrderError {
    Empty,
    TooManyLines,
    /// A line of 0 or more than [`MAX_ORDER_COUNT`].
    BadCount,
    NotSoldHere,
}

impl OrderError {
    pub const fn code(self) -> &'static str {
        match self {
            OrderError::Empty => "shop_empty",
            OrderError::TooManyLines => "shop_too_many",
            OrderError::BadCount => "shop_bad_count",
            OrderError::NotSoldHere => "not_sold_here",
        }
    }

    pub const fn message(self) -> &'static str {
        match self {
            OrderError::Empty => "The cart is empty.",
            OrderError::TooManyLines => "At most twenty different items to buy and to sell at once.",
            OrderError::BadCount => "From 1 to 999 of each item.",
            OrderError::NotSoldHere => "This merchant does not sell that.",
        }
    }
}

impl ShopOrder {
    /// The order's shape and the merchant's `stock` (the hub, at the
    /// merchant). The bag is checked when the order is applied.
    pub fn check(&self, stock: &[&str]) -> Result<(), OrderError> {
        if self.buy.is_empty() && self.sell.is_empty() {
            return Err(OrderError::Empty);
        }
        if self.buy.len() > MAX_ORDER_LINES || self.sell.len() > MAX_ORDER_LINES {
            return Err(OrderError::TooManyLines);
        }
        if self.buy.iter().chain(&self.sell).any(|(_, n)| !(1..=MAX_ORDER_COUNT).contains(n)) {
            return Err(OrderError::BadCount);
        }
        if self.buy.iter().any(|(id, _)| !stock.contains(&id.as_str())) {
            return Err(OrderError::NotSoldHere);
        }
        Ok(())
    }

    /// Applies a [checked](Self::check) order to `bag`: every sale, then
    /// every purchase. All or nothing: on an error `bag` is untouched.
    pub fn apply(&self, bag: &mut Bag) -> Result<Receipt, ItemError> {
        let mut next = bag.clone();
        let mut receipt = Receipt::default();
        for (id, n) in &self.sell {
            receipt.earned += next.sell(id, *n)?;
        }
        for (id, n) in &self.buy {
            receipt.spent += next.buy(id, *n, &[id.as_str()])?;
        }
        *bag = next;
        Ok(receipt)
    }

    /// Wire form (`protocol.rs`, 0x8C SHOP_DEAL, after the merchant): two
    /// lists of {str item, var count}, buys then sells.
    pub fn put(&self, b: &mut impl PutWire) {
        for lines in [&self.buy, &self.sell] {
            b.put_var(lines.len() as u64);
            for (id, n) in lines {
                b.put_str(id);
                b.put_var((*n).into());
            }
        }
    }

    pub fn read(r: &mut Reader<'_>) -> Option<Self> {
        let mut lines = || {
            let n = r.var()?;
            if n > MAX_ORDER_LINES as u64 {
                return None;
            }
            (0..n).map(|_| Some((r.str()?, r.var32()?))).collect::<Option<Vec<_>>>()
        };
        let buy = lines()?;
        let sell = lines()?;
        Some(ShopOrder { buy, sell })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::combat::items::{Attrs, Stack, MAX_STACK, MAX_STACKS};

    fn bag(zeny: u64, items: &[(&str, u32)]) -> Bag {
        let mut b = Bag { zeny, ..Bag::default() };
        for (id, n) in items {
            b.add(id, *n).unwrap();
        }
        b.equip[4] = "knife".into();
        b
    }

    fn offer(zeny: u64, items: &[(&str, u32)]) -> Offer {
        Offer { zeny, items: items.iter().map(|(i, n)| (i.to_string(), *n)).collect() }
    }

    #[test]
    fn the_exchange_moves_items_and_zeny_both_ways() {
        let a = bag(500, &[("red_potion", 5), ("jellopy", 3)]);
        let b = bag(100, &[("sword", 1)]);
        let (na, nb) = exchange(&a, &b, &offer(200, &[("red_potion", 2), ("jellopy", 3)]), &offer(0, &[("sword", 1)])).unwrap();
        assert_eq!((na.zeny, na.count("red_potion"), na.count("jellopy"), na.count("sword")), (300, 3, 0, 1));
        assert_eq!((nb.zeny, nb.count("red_potion"), nb.count("jellopy"), nb.count("sword")), (300, 2, 3, 0));
        assert_eq!(na.equip, a.equip, "what is worn never moves");
        // Nothing is created or lost.
        let total = |id: &str| na.count(id) + nb.count(id);
        assert_eq!((total("red_potion"), total("jellopy"), total("sword"), na.zeny + nb.zeny), (5, 3, 1, 600));
    }

    #[test]
    fn the_exchange_refuses_what_a_side_does_not_have() {
        let a = bag(50, &[("red_potion", 1)]);
        let b = bag(0, &[]);
        assert_eq!(exchange(&a, &b, &offer(51, &[]), &Offer::default()), Err(TradeError::NoZeny));
        assert_eq!(exchange(&a, &b, &offer(0, &[("red_potion", 2)]), &Offer::default()), Err(TradeError::NotOwned));
        // The worn knife is not in the bag's stacks.
        assert_eq!(exchange(&a, &b, &offer(0, &[("knife", 1)]), &Offer::default()), Err(TradeError::NotOwned));
        assert_eq!(exchange(&a, &b, &offer(0, &[("red_potion", 0)]), &Offer::default()), Err(TradeError::NotOwned));
    }

    #[test]
    fn the_exchange_checks_room_and_the_zeny_cap() {
        // A hundred different knives (refine × card): a hundred stacks.
        let knife = |i: usize| Attrs { refine: (i % 10) as u8 + 1, cards: vec![format!("card_{}", i / 10)] };
        let full = Bag { items: (0..MAX_STACKS).map(|i| Stack { id: "knife".into(), count: 1, attrs: knife(i) }).collect(), ..Bag::default() };
        let first = items::key("knife", &knife(0));
        assert_eq!(first, "knife+1[card_0]");
        let a = bag(0, &[("sword", 1)]);
        assert_eq!(exchange(&a, &full, &offer(0, &[("sword", 1)]), &Offer::default()), Err(TradeError::BagFull));
        // Giving a stack away frees the slot the incoming one needs.
        let (_, nb) = exchange(&a, &full, &offer(0, &[("sword", 1)]), &offer(0, &[(&first, 1)])).unwrap();
        assert_eq!((nb.count("sword"), nb.count(&first), nb.items.len()), (1, 0, MAX_STACKS));
        let stacked = bag(0, &[("red_potion", MAX_STACK)]);
        let more = bag(0, &[("red_potion", 1)]);
        assert_eq!(exchange(&more, &stacked, &offer(0, &[("red_potion", 1)]), &Offer::default()), Err(TradeError::BagFull));
        let rich = bag(MAX_ZENY, &[]);
        let some = bag(10, &[]);
        assert_eq!(exchange(&some, &rich, &offer(10, &[]), &Offer::default()), Err(TradeError::ZenyLimit));
    }

    #[test]
    fn offers_follow_the_bag() {
        let b = bag(100, &[("red_potion", 5), ("jellopy", 1)]);
        let mut o = Offer::default();
        assert_eq!(o.set_item(&b, "red_potion", 6), Err(TradeError::NotOwned));
        assert_eq!(o.set_item(&b, "knife", 1), Err(TradeError::NotOwned), "worn");
        assert_eq!(o.set_item(&b, "no_such_item", 1), Err(TradeError::NotOwned));
        o.set_item(&b, "red_potion", 5).unwrap();
        o.set_item(&b, "red_potion", 3).unwrap();
        assert_eq!(o.count("red_potion"), 3);
        assert_eq!(o.set_zeny(&b, 101), Err(TradeError::NoZeny));
        o.set_zeny(&b, 100).unwrap();
        assert!(o.covered_by(&b));
        let poorer = bag(40, &[("red_potion", 2)]);
        assert!(!o.covered_by(&poorer));
        assert!(o.clamp_to(&poorer));
        assert_eq!(o, offer(40, &[("red_potion", 2)]));
        assert!(!o.clamp_to(&poorer));
        o.set_item(&b, "red_potion", 0).unwrap();
        assert_eq!(o.count("red_potion"), 0);
    }

    /// Instances trade by their key: part of a stack splits off and lands
    /// on the receiver's matching stack; a refined copy never merges with
    /// a plain one, and nothing is created or lost.
    #[test]
    fn stacks_split_and_merge_by_instance() {
        let a = bag(0, &[("sword", 3), ("sword+7", 2), ("red_potion", 4)]);
        let b = bag(0, &[("sword", 1), ("sword+7[poring_card]", 1)]);
        let mut o = Offer::default();
        o.set_item(&a, "sword+7", 1).unwrap();
        o.set_item(&a, "sword", 2).unwrap();
        assert_eq!(o.set_item(&a, "sword+7", 3), Err(TradeError::NotOwned));
        // Aliases of a key are refused (one spelling per instance).
        assert_eq!(o.set_item(&a, "sword+07", 1), Err(TradeError::NotOwned));
        let (na, nb) = exchange(&a, &b, &o, &offer(0, &[("sword+7[poring_card]", 1)])).unwrap();
        assert_eq!((na.count("sword"), na.count("sword+7"), na.count("sword+7[poring_card]")), (1, 1, 1));
        assert_eq!((nb.count("sword"), nb.count("sword+7"), nb.count("sword+7[poring_card]")), (3, 1, 0));
        for k in ["sword", "sword+7", "sword+7[poring_card]", "red_potion"] {
            assert_eq!(na.count(k) + nb.count(k), a.count(k) + b.count(k), "{k}");
        }
        // One stack per instance on each side.
        assert_eq!(nb.items.len(), 2);
    }

    #[test]
    fn at_most_ten_different_items() {
        let ids: Vec<&str> = items::catalogue().iter().map(|e| e.id).take(MAX_OFFER_ITEMS + 1).collect();
        let b = bag(0, &ids.iter().map(|i| (*i, 1)).collect::<Vec<_>>());
        let mut o = Offer::default();
        for id in &ids[..MAX_OFFER_ITEMS] {
            o.set_item(&b, id, 1).unwrap();
        }
        assert_eq!(o.set_item(&b, ids[MAX_OFFER_ITEMS], 1), Err(TradeError::TooManyItems));
    }

    fn order(buy: &[(&str, u32)], sell: &[(&str, u32)]) -> ShopOrder {
        let lines = |l: &[(&str, u32)]| l.iter().map(|(i, n)| (i.to_string(), *n)).collect();
        ShopOrder { buy: lines(buy), sell: lines(sell) }
    }

    #[test]
    fn a_shop_order_sells_first_then_buys_all_or_nothing() {
        let price = |id: &str| u64::from(items::find(id).unwrap().price);
        let sale = |id: &str| u64::from(items::sell_price(items::find(id).unwrap().price));
        // The jellopy sold pays for part of the potions.
        let start = bag(2 * price("red_potion") - 4 * sale("jellopy"), &[("jellopy", 10)]);
        let mut b = start.clone();
        let o = order(&[("red_potion", 2)], &[("jellopy", 4)]);
        let r = o.apply(&mut b).unwrap();
        assert_eq!(r, Receipt { spent: 2 * price("red_potion"), earned: 4 * sale("jellopy") });
        assert_eq!((b.zeny, b.count("red_potion"), b.count("jellopy")), (0, 2, 6));
        // One zeny short, or one jellopy more than owned: nothing moves.
        for bad in [order(&[("red_potion", 3)], &[("jellopy", 4)]), order(&[("red_potion", 1)], &[("jellopy", 11)])] {
            let mut b = start.clone();
            assert!(bad.apply(&mut b).is_err(), "{bad:?}");
            assert_eq!(b, start, "{bad:?}");
        }
        // The worn knife is not in the bag: it cannot be sold.
        let mut b = start.clone();
        assert_eq!(order(&[], &[("knife", 1)]).apply(&mut b), Err(ItemError::NotInBag));
        // Selling frees the room a full bag needs for a purchase.
        let mut full = Bag::default();
        full.zeny = price("red_potion");
        let others: Vec<&str> = items::catalogue().iter().map(|e| e.id).filter(|id| *id != "red_potion").take(3).collect();
        full.items = (0..MAX_STACKS).map(|i| Stack { id: others[i % 3].into(), count: 1, attrs: Attrs { refine: (i / 3) as u8, ..Attrs::default() } }).collect();
        let key = full.items[0].key();
        let mut b = full.clone();
        assert_eq!(order(&[("red_potion", 1)], &[]).apply(&mut b), Err(ItemError::BagFull));
        assert!(order(&[("red_potion", 1)], &[(key.as_str(), 1)]).apply(&mut b).is_ok());
    }

    #[test]
    fn shop_orders_are_checked_against_the_stock_and_their_shape() {
        let stock = ["red_potion", "blue_potion"];
        assert_eq!(order(&[("red_potion", 999)], &[("jellopy", 1)]).check(&stock), Ok(()));
        assert_eq!(order(&[], &[]).check(&stock), Err(OrderError::Empty));
        assert_eq!(order(&[("sword", 1)], &[]).check(&stock), Err(OrderError::NotSoldHere));
        assert_eq!(order(&[("red_potion", 0)], &[]).check(&stock), Err(OrderError::BadCount));
        assert_eq!(order(&[], &[("jellopy", MAX_ORDER_COUNT + 1)]).check(&stock), Err(OrderError::BadCount));
        let many: Vec<(&str, u32)> = (0..=MAX_ORDER_LINES).map(|_| ("jellopy", 1)).collect();
        assert_eq!(order(&[], &many).check(&stock), Err(OrderError::TooManyLines));
        // The wire form bounds the lists too.
        let mut w = bytes::BytesMut::new();
        order(&[], &many).put(&mut w);
        assert!(ShopOrder::read(&mut Reader::new(&w)).is_none());
    }
}

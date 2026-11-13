//! Primitives of the binary wire protocol (WebSocket binary frames).
//!
//! Every frame starts with a one-byte message kind (see
//! [`super::protocol`]); `0xFF` is reserved as an escape: the next byte is
//! an extended kind, for when the 254 single-byte kinds run out. The body
//! is a fixed field sequence per kind, little-endian, MessagePack-like but
//! schema-driven (no field names or type tags on the wire):
//!
//! | type      | encoding                                                    |
//! |-----------|-------------------------------------------------------------|
//! | `var`     | unsigned LEB128 varint (1 byte below 128)                   |
//! | `zig`     | signed varint, zigzag-mapped                                |
//! | `str`     | `var` byte length, UTF-8 bytes                              |
//! | `opt str` | `var` length + 1, 0 = absent                                |
//! | `code`    | `u8` index into [`CODES`]; `0xFF` + `str` for anything else |
//! | `pos`     | `u16` cells, fixed point with 6 fractional bits             |
//! | `bool`    | `u8` 0 / 1                                                  |
//! | `f32`     | IEEE 754, little-endian                                     |
//!
//! [`Reader`] returns `None` on any truncated or malformed field; decoders
//! turn that into "ignore the frame".

use bytes::BufMut;

/// Escape byte: an extended (two-byte) message kind follows.
pub const EXTENDED_KIND: u8 = 0xFF;

/// Error codes and reasons sent as one byte. Append only: the index is the
/// wire value (the Dart client keeps the same table).
pub const CODES: &[&str] = &[
    "bad_message",
    "hub_unavailable",
    "no_character",
    "hub_busy",
    "not_in_hub",
    "no_such_npc",
    "npc_too_far",
    "unknown_mission",
    "evicted",
    "requested",
    "replaced",
    "kicked",
    "launched",
    "left",
    "disbanded",
    "disconnected",
    "stage_requires_mission",
    "already_in_lobby",
    "not_in_lobby",
    "no_such_lobby",
    "lobby_full",
    "not_leader",
    "not_ready",
    "stage_full",
    "mission_unavailable",
    "no_such_member",
    "stage_busy",
    "stage_gone",
    "channel_full",
    "hub_full",
    "no_such_channel",
    "not_in_stage",
];

const CODE_ESCAPE: u8 = 0xFF;
const FIXED_ONE: f32 = 64.0;

pub fn encode_pos(v: f32) -> u16 {
    (v * FIXED_ONE).round().clamp(0.0, u16::MAX as f32) as u16
}

pub fn decode_pos(v: u16) -> f32 {
    v as f32 / FIXED_ONE
}

/// Writers for the protocol types on any [`BufMut`].
pub trait PutWire: BufMut {
    fn put_var(&mut self, mut v: u64) {
        while v >= 0x80 {
            self.put_u8(v as u8 | 0x80);
            v >>= 7;
        }
        self.put_u8(v as u8);
    }

    fn put_zig(&mut self, v: i64) {
        self.put_var(((v << 1) ^ (v >> 63)) as u64);
    }

    fn put_str(&mut self, s: &str) {
        self.put_var(s.len() as u64);
        self.put_slice(s.as_bytes());
    }

    fn put_opt_str(&mut self, s: Option<&str>) {
        match s {
            Some(s) => {
                self.put_var(s.len() as u64 + 1);
                self.put_slice(s.as_bytes());
            }
            None => self.put_u8(0),
        }
    }

    fn put_code(&mut self, code: &str) {
        match CODES.iter().position(|c| *c == code) {
            Some(i) => self.put_u8(i as u8),
            None => {
                self.put_u8(CODE_ESCAPE);
                self.put_str(code);
            }
        }
    }

    fn put_pos(&mut self, v: f32) {
        self.put_u16_le(encode_pos(v));
    }

    fn put_bool(&mut self, v: bool) {
        self.put_u8(v as u8);
    }
}

impl<T: BufMut + ?Sized> PutWire for T {}

/// Cursor over one frame.
pub struct Reader<'a> {
    b: &'a [u8],
    at: usize,
}

impl<'a> Reader<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        Self { b, at: 0 }
    }

    pub fn is_empty(&self) -> bool {
        self.at >= self.b.len()
    }

    pub fn remaining(&self) -> usize {
        self.b.len().saturating_sub(self.at)
    }

    pub fn bytes(&mut self, n: usize) -> Option<&'a [u8]> {
        let end = self.at.checked_add(n)?;
        let s = self.b.get(self.at..end)?;
        self.at = end;
        Some(s)
    }

    pub fn u8(&mut self) -> Option<u8> {
        let v = *self.b.get(self.at)?;
        self.at += 1;
        Some(v)
    }

    pub fn i8(&mut self) -> Option<i8> {
        self.u8().map(|v| v as i8)
    }

    pub fn u16(&mut self) -> Option<u16> {
        self.bytes(2).map(|s| u16::from_le_bytes([s[0], s[1]]))
    }

    pub fn u32(&mut self) -> Option<u32> {
        self.bytes(4).map(|s| u32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> Option<f32> {
        self.bytes(4).map(|s| f32::from_le_bytes(s.try_into().unwrap()))
    }

    pub fn var(&mut self) -> Option<u64> {
        let mut v = 0u64;
        for shift in (0..64).step_by(7) {
            let b = self.u8()?;
            v |= u64::from(b & 0x7F) << shift;
            if b < 0x80 {
                return Some(v);
            }
        }
        None
    }

    /// A varint that must fit `u32`.
    pub fn var32(&mut self) -> Option<u32> {
        self.var().and_then(|v| u32::try_from(v).ok())
    }

    pub fn zig(&mut self) -> Option<i64> {
        let v = self.var()?;
        Some((v >> 1) as i64 ^ -((v & 1) as i64))
    }

    pub fn str(&mut self) -> Option<String> {
        let n = usize::try_from(self.var()?).ok()?;
        std::str::from_utf8(self.bytes(n)?).ok().map(str::to_owned)
    }

    pub fn opt_str(&mut self) -> Option<Option<String>> {
        let n = usize::try_from(self.var()?).ok()?;
        if n == 0 {
            return Some(None);
        }
        std::str::from_utf8(self.bytes(n - 1)?).ok().map(|s| Some(s.to_owned()))
    }

    pub fn code(&mut self) -> Option<String> {
        match self.u8()? {
            CODE_ESCAPE => self.str(),
            i => CODES.get(i as usize).map(|c| (*c).to_owned()),
        }
    }

    pub fn pos(&mut self) -> Option<f32> {
        self.u16().map(decode_pos)
    }

    pub fn bool(&mut self) -> Option<bool> {
        match self.u8()? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }

    /// Message kind: one byte, or the escape and an extended byte
    /// (returned as `0xFF00 | ext`).
    pub fn kind(&mut self) -> Option<u16> {
        match self.u8()? {
            EXTENDED_KIND => self.u8().map(|e| 0xFF00 | u16::from(e)),
            k => Some(u16::from(k)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::BytesMut;

    #[test]
    fn primitives_round_trip() {
        let mut b = BytesMut::new();
        for v in [0u64, 1, 127, 128, 300, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            b.put_var(v);
        }
        for v in [0i64, -1, 1, -64, 63, -65, i32::MIN as i64, i64::MAX, i64::MIN] {
            b.put_zig(v);
        }
        b.put_str("Poring ✓");
        b.put_opt_str(None);
        b.put_opt_str(Some(""));
        b.put_opt_str(Some("bgm"));
        b.put_code("kicked");
        b.put_code("brand_new_reason");
        b.put_pos(133.25);
        b.put_bool(true);
        b.put_f32_le(1.5);
        let mut r = Reader::new(&b);
        for v in [0u64, 1, 127, 128, 300, 16_383, 16_384, u32::MAX as u64, u64::MAX] {
            assert_eq!(r.var(), Some(v));
        }
        for v in [0i64, -1, 1, -64, 63, -65, i32::MIN as i64, i64::MAX, i64::MIN] {
            assert_eq!(r.zig(), Some(v));
        }
        assert_eq!(r.str().as_deref(), Some("Poring ✓"));
        assert_eq!(r.opt_str(), Some(None));
        assert_eq!(r.opt_str(), Some(Some(String::new())));
        assert_eq!(r.opt_str(), Some(Some("bgm".into())));
        assert_eq!(r.code().as_deref(), Some("kicked"));
        assert_eq!(r.code().as_deref(), Some("brand_new_reason"));
        assert_eq!(r.pos(), Some(133.25));
        assert_eq!(r.bool(), Some(true));
        assert_eq!(r.f32(), Some(1.5));
        assert!(r.is_empty());
        assert_eq!(r.u8(), None);
    }

    #[test]
    fn small_values_are_one_byte() {
        let mut b = BytesMut::new();
        b.put_var(127);
        b.put_zig(-64);
        b.put_code("kicked");
        assert_eq!(b.len(), 3);
    }

    #[test]
    fn truncated_and_malformed_fields_fail() {
        assert_eq!(Reader::new(&[0x80]).var(), None, "unterminated varint");
        assert_eq!(Reader::new(&[0xFF; 11]).var(), None, "varint longer than 64 bits");
        assert_eq!(Reader::new(&[5, b'a']).str(), None, "short string");
        assert_eq!(Reader::new(&[2, 0xC3, 0x28]).str(), None, "invalid UTF-8");
        assert_eq!(Reader::new(&[2]).bool(), None);
        assert_eq!(Reader::new(&[200]).code(), None, "unknown code index");
        assert_eq!(Reader::new(&[0xFF, 7]).kind(), Some(0xFF07));
        assert_eq!(Reader::new(&[0xFF]).kind(), None);
    }

    #[test]
    fn code_table_fits_below_the_escape() {
        assert!(CODES.len() < CODE_ESCAPE as usize);
        let mut seen = std::collections::HashSet::new();
        assert!(CODES.iter().all(|c| seen.insert(c)), "codes are unique");
    }
}

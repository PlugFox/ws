//! `/raw?hex=&preset=&chunk=0&delay_ms=0&end=wait|fin|rst|hold`: writes
//! scripted bytes after the `101`. Presets keep URLs short and identical
//! across transports.

use std::{io, time::Duration};

use crate::{
    frame::{
        End, OP_BINARY, OP_CLOSE, OP_CONT, OP_PING, OP_TEXT, Session, close_payload, encode,
        encode_len,
    },
    http::{Query, hex_decode},
};

/// Bytes to write and the `end` mode used when the URL gives none.
pub struct Script {
    pub bytes: Vec<u8>,
    pub end: End,
}

/// Builds the script for a request, or an error message for a `400`.
pub fn script(q: &Query) -> Result<Script, String> {
    let mut script = Script {
        bytes: Vec::new(),
        end: End::Wait,
    };
    if let Some(name) = q.get("preset") {
        let (bytes, end) = preset(name).ok_or_else(|| format!("unknown preset {name}"))?;
        script.bytes = bytes;
        script.end = end;
    }
    if let Some(hex) = q.get("hex") {
        script
            .bytes
            .extend(hex_decode(hex).ok_or_else(|| format!("bad hex {hex}"))?);
    }
    script.end = End::parse(q.get("end"), script.end);
    Ok(script)
}

/// Writes the script in `chunk`-byte pieces (`0` = one write), `delay_ms`
/// apart, then ends as requested.
pub async fn raw(mut s: Session, q: &Query, script: Script) -> io::Result<()> {
    let chunk = match q.num("chunk", 0) as usize {
        0 => script.bytes.len().max(1),
        n => n,
    };
    let delay = Duration::from_millis(q.num("delay_ms", 0));
    for (i, piece) in script.bytes.chunks(chunk).enumerate() {
        if i > 0 && !delay.is_zero() {
            tokio::time::sleep(delay).await;
        }
        s.write(piece).await?;
    }
    s.finish(script.end).await;
    Ok(())
}

/// One unmasked frame.
fn frame(fin: bool, rsv: u8, op: u8, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    encode(&mut out, fin, rsv, op, payload);
    out
}

/// The named preset's bytes and default end mode.
pub fn preset(name: &str) -> Option<(Vec<u8>, End)> {
    if let Some(code) = name.strip_prefix("close-code-") {
        let code: u16 = code.parse().ok()?;
        return Some((
            frame(true, 0, OP_CLOSE, &close_payload(Some(code), "")),
            End::Wait,
        ));
    }
    let bytes = match name {
        // Header bits and opcodes.
        "rsv1" => frame(true, 4, OP_TEXT, b"hi"),
        "rsv2" => frame(true, 2, OP_TEXT, b"hi"),
        "rsv3" => frame(true, 1, OP_TEXT, b"hi"),
        "opcode3" => frame(true, 0, 0x3, b""),
        "opcodeB" => frame(true, 0, 0xB, b""),
        "masked" => masked_text(b"hi"),
        // Control frames.
        "ping-nofin" => frame(false, 0, OP_PING, b""),
        "ping126" => frame(true, 0, OP_PING, &[0x70; 126]),
        // Fragmentation.
        "cont-first" => frame(true, 0, OP_CONT, b"hi"),
        "text-mid-frag" => [
            frame(false, 0, OP_TEXT, b"he"),
            frame(true, 0, OP_TEXT, b"lo"),
        ]
        .concat(),
        // Valid: a close interleaved in an unfinished message.
        "close-mid-frag" => [
            frame(false, 0, OP_TEXT, b"he"),
            frame(true, 0, OP_CLOSE, &close_payload(Some(1000), "")),
        ]
        .concat(),
        // Close frames.
        "close1b" => frame(true, 0, OP_CLOSE, &[0x03]),
        "close-bad-utf8" => frame(true, 0, OP_CLOSE, &[0x03, 0xE8, 0xFF, 0xFE]),
        // UTF-8.
        "utf8-bad" => frame(true, 0, OP_TEXT, b"ab\xFF"),
        "utf8-trunc" => frame(true, 0, OP_TEXT, b"ab\xE2\x82"),
        "utf8-frag-bad" => [
            frame(false, 0, OP_TEXT, b"ab\xE2\x82"),
            frame(true, 0, OP_CONT, b"\x28"),
        ]
        .concat(),
        // Lengths.
        "len64-msb" => long_len(0x8000_0000_0000_0005, b"hello"),
        "len64-max" => long_len(u64::MAX >> 1, b""),
        "len-nonmin" => [&[0x80 | OP_BINARY, 126, 0, 5][..], b"hello"].concat(),
        // Truncation and limits.
        "eof-in-header" => return Some((vec![0x80 | OP_BINARY], End::Fin)),
        "eof-in-payload" => {
            return Some(([&[0x80 | OP_BINARY, 10][..], b"abc"].concat(), End::Fin));
        }
        "eof-between-frags" => return Some((frame(false, 0, OP_TEXT, b"he"), End::Fin)),
        "zero-cont-x10000" => zero_conts(10_000),
        _ => return None,
    };
    Some((bytes, End::Wait))
}

/// A text frame with the mask bit set (servers must not mask).
fn masked_text(payload: &[u8]) -> Vec<u8> {
    let mask = [0x11, 0x22, 0x33, 0x44];
    let mut out = vec![0x80 | OP_TEXT];
    encode_len(&mut out, payload.len() as u64, true);
    out.extend(mask);
    out.extend(payload.iter().enumerate().map(|(i, b)| b ^ mask[i % 4]));
    out
}

/// A binary frame header with a 64-bit length field holding `len`.
fn long_len(len: u64, payload: &[u8]) -> Vec<u8> {
    let mut out = vec![0x80 | OP_BINARY, 127];
    out.extend(len.to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// An empty text message split into `n` empty continuation frames.
fn zero_conts(n: usize) -> Vec<u8> {
    let mut out = frame(false, 0, OP_TEXT, b"");
    for _ in 0..n {
        out.extend([OP_CONT, 0]);
    }
    out.extend(frame(true, 0, OP_CONT, b""));
    out
}

/// Every preset name (`close-code-<n>` shown as `close-code-1000`).
pub const PRESETS: &[&str] = &[
    "rsv1",
    "rsv2",
    "rsv3",
    "opcode3",
    "opcodeB",
    "masked",
    "ping-nofin",
    "ping126",
    "cont-first",
    "text-mid-frag",
    "close-mid-frag",
    "close1b",
    "close-code-1000",
    "close-bad-utf8",
    "utf8-bad",
    "utf8-trunc",
    "utf8-frag-bad",
    "len64-msb",
    "len64-max",
    "len-nonmin",
    "eof-in-header",
    "eof-in-payload",
    "eof-between-frags",
    "zero-cont-x10000",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_preset_exists() {
        for name in PRESETS {
            assert!(preset(name).is_some(), "{name}");
        }
        assert!(preset("nope").is_none());
        assert_eq!(preset("rsv1").unwrap().0, [0xC1, 2, b'h', b'i']);
        assert_eq!(preset("close-code-3000").unwrap().0, [0x88, 2, 0x0B, 0xB8]);
    }
}

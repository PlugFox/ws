//! Converts JSON into the tape format read by `lib/src/tape.dart` (see that
//! file for the format).

use simd_json::{Node, StaticNode};

const TAG_NULL: u64 = 0x6E;
const TAG_TRUE: u64 = 0x74;
const TAG_FALSE: u64 = 0x66;
const TAG_INT: u64 = 0x6C;
const TAG_DOUBLE: u64 = 0x64;
const TAG_STRING: u64 = 0x73;
const TAG_OBJECT: u64 = 0x7B;
const TAG_ARRAY: u64 = 0x5B;
const ASCII_FLAG: u64 = 1 << 62;
const VERSION: u64 = 1;

/// A finished tape buffer: `bytes` valid bytes inside an 8-byte aligned
/// allocation (so Dart can view it as `Int64List`).
pub struct TapeBuffer {
    pub words: Vec<u64>,
    pub bytes: usize,
}

/// Parses `input` (mutated in place by simd-json) into a tape buffer.
pub fn build_tape(mut input: Vec<u8>) -> Result<TapeBuffer, String> {
    let base = input.as_ptr() as usize;
    let len = input.len();
    let tape = simd_json::to_tape(&mut input).map_err(|e| e.to_string())?;
    let nodes = &tape.0;

    // words[0] is the header; tape index = vec index - 1.
    let mut words: Vec<u64> = Vec::with_capacity(nodes.len() * 2 + 1);
    words.push(0);
    let mut extra: Vec<u8> = Vec::new();
    // Open containers: (vec index of the container word, items remaining).
    let mut stack: Vec<(usize, usize)> = Vec::with_capacity(16);

    fn close_completed(words: &mut [u64], stack: &mut Vec<(usize, usize)>, tape_len: usize) {
        while let Some(top) = stack.last_mut() {
            top.1 -= 1;
            if top.1 != 0 {
                return;
            }
            let (start, _) = stack.pop().unwrap();
            words[start] |= tape_len as u64;
        }
    }

    for node in nodes.iter() {
        match *node {
            Node::String(s) => {
                let p = s.as_ptr() as usize;
                let offset = if p >= base && p + s.len() <= base + len {
                    p - base
                } else {
                    let o = len + extra.len();
                    extra.extend_from_slice(s.as_bytes());
                    o
                };
                words.push((TAG_STRING << 56) | offset as u64);
                words.push(s.len() as u64 | if s.is_ascii() { ASCII_FLAG } else { 0 });
            }
            Node::Object { len: n, .. } | Node::Array { len: n, .. } => {
                let (tag, items) = match node {
                    Node::Object { .. } => (TAG_OBJECT, n * 2),
                    _ => (TAG_ARRAY, n),
                };
                let start = words.len();
                words.push(tag << 56);
                words.push(n as u64);
                if items > 0 {
                    stack.push((start, items));
                    continue;
                }
                words[start] |= (words.len() - 1) as u64;
            }
            Node::Static(s) => match s {
                StaticNode::Null => words.push(TAG_NULL << 56),
                StaticNode::Bool(true) => words.push(TAG_TRUE << 56),
                StaticNode::Bool(false) => words.push(TAG_FALSE << 56),
                StaticNode::I64(v) => {
                    words.push(TAG_INT << 56);
                    words.push(v as u64);
                }
                StaticNode::U64(v) if v <= i64::MAX as u64 => {
                    words.push(TAG_INT << 56);
                    words.push(v);
                }
                StaticNode::U64(v) => {
                    words.push(TAG_DOUBLE << 56);
                    words.push((v as f64).to_bits());
                }
                StaticNode::F64(v) => {
                    words.push(TAG_DOUBLE << 56);
                    words.push(v.to_bits());
                }
            },
        }
        let tape_len = words.len() - 1;
        close_completed(&mut words, &mut stack, tape_len);
    }

    let n = words.len() - 1;
    words[0] = (VERSION << 32) | n as u64;
    let header_bytes = words.len() * 8;
    let total = header_bytes + len + extra.len();
    words.resize(total.div_ceil(8), 0);
    // SAFETY: the vector holds at least `total` initialized bytes.
    let bytes = unsafe { std::slice::from_raw_parts_mut(words.as_mut_ptr() as *mut u8, total) };
    drop(tape);
    bytes[header_bytes..header_bytes + len].copy_from_slice(&input);
    bytes[header_bytes + len..].copy_from_slice(&extra);
    Ok(TapeBuffer {
        words,
        bytes: total,
    })
}

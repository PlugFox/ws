//! Local chat: what players may say and how often.
//!
//! A line goes to the players around the speaker (`HubInstance::chat`):
//! within [`CHAT_RANGE`] cells on a hub map (about one screen, as RO's
//! visible area), the whole map on a stage (the party). The client shows it
//! over the speaker's head and in its chat log. Server announcements (the
//! admin API) reach everyone connected.

use std::time::{Duration, Instant};

/// Longest line, in characters (longer ones are cut).
pub const CHAT_MAX_CHARS: usize = 120;
/// Hub maps: players within this many cells of the speaker hear them
/// (inside the town's snapshot sight, so the speaker is on screen).
pub const CHAT_RANGE: f32 = 30.0;
/// Rate limit: a burst of [`CHAT_BURST`] lines, then one per
/// [`CHAT_INTERVAL`].
pub const CHAT_BURST: u32 = 4;
pub const CHAT_INTERVAL: Duration = Duration::from_millis(1500);

/// Cleans a chat line: control and bidi-override characters go, runs of
/// whitespace become one space, the ends are trimmed and the line is cut to
/// [`CHAT_MAX_CHARS`]. `None` when nothing is left.
pub fn sanitize(text: &str) -> Option<String> {
    clean(text, CHAT_MAX_CHARS)
}

/// [`sanitize`] with another length cap (announcements).
pub fn clean(text: &str, max_chars: usize) -> Option<String> {
    let mut out = String::with_capacity(text.len().min(max_chars * 4));
    let mut chars = 0;
    let mut space = false;
    for c in text.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        if c.is_control() || is_format_trick(c) {
            continue;
        }
        if space {
            if chars + 1 >= max_chars {
                break;
            }
            out.push(' ');
            chars += 1;
            space = false;
        }
        if chars >= max_chars {
            break;
        }
        out.push(c);
        chars += 1;
    }
    (!out.is_empty()).then_some(out)
}

/// Invisible characters that reorder or hide text (bidi overrides and
/// isolates, zero-width marks): a line must read as it is written.
pub fn is_format_trick(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}'..='\u{206F}' | '\u{FEFF}')
}

/// Per-socket chat rate limit (generic cell rate: a burst of
/// [`CHAT_BURST`], refilled one line per [`CHAT_INTERVAL`]). Kept by the
/// socket, so it survives channel switches and stage trips.
#[derive(Debug, Default)]
pub struct ChatLimiter {
    /// Theoretical arrival time of the next line.
    tat: Option<Instant>,
}

impl ChatLimiter {
    /// Whether a line sent at `now` may go out; counts it if so.
    pub fn allow(&mut self, now: Instant) -> bool {
        let tat = self.tat.map_or(now, |t| t.max(now));
        let slack = CHAT_INTERVAL * (CHAT_BURST - 1);
        if tat.saturating_duration_since(now) > slack {
            return false;
        }
        self.tat = Some(tat + CHAT_INTERVAL);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_strips_controls_collapses_space_and_caps_length() {
        assert_eq!(sanitize("  hello \t\n  world  ").as_deref(), Some("hello world"));
        assert_eq!(sanitize("a\u{0007}b\u{001b}[31mc").as_deref(), Some("ab[31mc"));
        assert_eq!(sanitize("evil\u{202E}txt.exe\u{200B}").as_deref(), Some("eviltxt.exe"));
        assert_eq!(sanitize(" \u{0000}\r\n "), None);
        assert_eq!(sanitize(""), None);
        assert_eq!(sanitize("Привет, 世界 👋").as_deref(), Some("Привет, 世界 👋"));
        let long = "ж".repeat(500);
        assert_eq!(sanitize(&long).unwrap().chars().count(), CHAT_MAX_CHARS);
        // A space is never the last character after the cut.
        let words = "ab ".repeat(100);
        let cut = sanitize(&words).unwrap();
        assert!(cut.chars().count() <= CHAT_MAX_CHARS && !cut.ends_with(' '), "{cut:?}");
    }

    #[test]
    fn limiter_allows_a_burst_then_one_per_interval() {
        let t0 = Instant::now();
        let mut l = ChatLimiter::default();
        for i in 0..CHAT_BURST {
            assert!(l.allow(t0), "burst line {i}");
        }
        assert!(!l.allow(t0), "over the burst");
        assert!(!l.allow(t0 + CHAT_INTERVAL / 2), "not refilled yet");
        assert!(l.allow(t0 + CHAT_INTERVAL), "one refilled");
        assert!(!l.allow(t0 + CHAT_INTERVAL));
        // A long pause refills the whole burst, no more.
        let later = t0 + CHAT_INTERVAL * 100;
        for _ in 0..CHAT_BURST {
            assert!(l.allow(later));
        }
        assert!(!l.allow(later));
    }
}

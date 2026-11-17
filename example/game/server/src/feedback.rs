//! Player feedback: bug reports and suggestions left at the Scribe (NPC
//! kind [`FEEDBACK_NPC`]) or a Kafra employee, stored in the `feedback`
//! table (`db::feedback_repo`) and worked through by operators or a coding
//! agent over MCP (`api::mcp`).
//!
//! The flow: the client sends `FEEDBACK` (protocol 0x5C); the socket
//! checks the form ([`Draft::validate`]), the hub instance checks that the
//! player stands at a Scribe or a Kafra and adds where they are, then the
//! socket applies the per-account rate limit ([`check_rate`]), stores the
//! report and answers `FEEDBACK_RESULT` (0x62).

use crate::world::chat::is_format_trick;
use std::fmt;

/// NPC kind of the Scribe: only takes feedback (a Kafra also does).
pub const FEEDBACK_NPC: &str = "feedback";

/// What a report is about (wire and DB value).
pub const KINDS: &[&str] = &["bug", "suggestion", "balance", "other"];
/// How much it matters to the player (wire and DB value).
pub const PRIORITIES: &[&str] = &["low", "normal", "high", "critical"];
/// `status` values.
pub const OPEN: &str = "open";
pub const CLOSED: &str = "closed";

/// Title length in characters (after cleaning).
pub const TITLE_MIN: usize = 4;
pub const TITLE_MAX: usize = 80;
/// Body length in characters (after cleaning).
pub const BODY_MIN: usize = 20;
pub const BODY_MAX: usize = 4000;
/// Context strings from the client are cut to these lengths.
pub const CLIENT_VERSION_MAX: usize = 40;
pub const USER_AGENT_MAX: usize = 300;
/// Operator's resolution note.
pub const RESOLUTION_MAX: usize = 2000;

/// Per account: at least this long between two reports, and at most
/// [`MAX_PER_DAY`] in any 24 hours.
pub const MIN_GAP_SECS: i64 = 60;
pub const MAX_PER_DAY: i64 = 10;

/// The form as the player filled it (cleaned by [`Draft::validate`]).
#[derive(Clone, Debug, PartialEq)]
pub struct Draft {
    pub kind: String,
    pub priority: String,
    pub title: String,
    pub body: String,
    /// Client build (`0.27 (2026-11-14)`), informational.
    pub client_version: String,
    /// The browser's user agent, informational.
    pub user_agent: String,
}

/// Where the player was when they sent it (filled by the hub instance).
#[derive(Clone, Debug, PartialEq)]
pub struct Place {
    /// Map id.
    pub map: String,
    pub x: f32,
    pub y: f32,
    pub level: u32,
    /// Archetype (`swordsman`, ...).
    pub class: String,
}

/// Why a report was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeedbackError {
    BadKind,
    BadPriority,
    TitleLength,
    BodyLength,
    /// Sent another report less than [`MIN_GAP_SECS`] ago (seconds to wait).
    TooSoon(i64),
    DailyLimit,
    /// Not at a Scribe or a Kafra.
    TooFar,
    Storage,
}

impl FeedbackError {
    /// Wire code (`FEEDBACK_RESULT`).
    pub fn code(self) -> &'static str {
        match self {
            FeedbackError::BadKind => "feedback_kind",
            FeedbackError::BadPriority => "feedback_priority",
            FeedbackError::TitleLength => "feedback_title",
            FeedbackError::BodyLength => "feedback_body",
            FeedbackError::TooSoon(_) => "feedback_too_soon",
            FeedbackError::DailyLimit => "feedback_daily_limit",
            FeedbackError::TooFar => "npc_too_far",
            FeedbackError::Storage => "feedback_storage",
        }
    }
}

impl fmt::Display for FeedbackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            FeedbackError::BadKind => write!(f, "Pick what the report is about."),
            FeedbackError::BadPriority => write!(f, "Pick a priority."),
            FeedbackError::TitleLength => write!(f, "The title needs {TITLE_MIN} to {TITLE_MAX} characters."),
            FeedbackError::BodyLength => {
                write!(f, "Describe it in {BODY_MIN} to {BODY_MAX} characters: what happened, what you expected, what you suggest.")
            }
            FeedbackError::TooSoon(s) => write!(f, "You just sent a report. Try again in {s} s."),
            FeedbackError::DailyLimit => write!(f, "That is {MAX_PER_DAY} reports today: thank you! Come back tomorrow."),
            FeedbackError::TooFar => write!(f, "Talk to the Scribe or a Kafra employee to leave feedback."),
            FeedbackError::Storage => write!(f, "The report could not be stored. Please try again later."),
        }
    }
}

impl Draft {
    /// Cleans the form and checks it: known kind and priority, title and
    /// body within their lengths (control and invisible characters
    /// dropped; the body keeps its line breaks, at most one blank line in a
    /// row), context strings cut to size.
    pub fn validate(self) -> Result<Draft, FeedbackError> {
        if !KINDS.contains(&self.kind.as_str()) {
            return Err(FeedbackError::BadKind);
        }
        if !PRIORITIES.contains(&self.priority.as_str()) {
            return Err(FeedbackError::BadPriority);
        }
        let title = clean_line(&self.title, TITLE_MAX + 1);
        if !(TITLE_MIN..=TITLE_MAX).contains(&title.chars().count()) {
            return Err(FeedbackError::TitleLength);
        }
        let body = clean_text(&self.body, BODY_MAX + 1);
        if !(BODY_MIN..=BODY_MAX).contains(&body.chars().count()) {
            return Err(FeedbackError::BodyLength);
        }
        Ok(Draft {
            kind: self.kind,
            priority: self.priority,
            title,
            body,
            client_version: clean_line(&self.client_version, CLIENT_VERSION_MAX),
            user_agent: clean_line(&self.user_agent, USER_AGENT_MAX),
        })
    }
}

/// The rate limit, given the account's reports of the last 24 hours and
/// how many seconds ago the latest was sent.
pub fn check_rate(last_day: i64, last_secs_ago: Option<i64>) -> Result<(), FeedbackError> {
    if let Some(ago) = last_secs_ago
        && ago < MIN_GAP_SECS
    {
        return Err(FeedbackError::TooSoon((MIN_GAP_SECS - ago).max(1)));
    }
    if last_day >= MAX_PER_DAY {
        return Err(FeedbackError::DailyLimit);
    }
    Ok(())
}

fn keep(c: char) -> bool {
    !c.is_control() && !is_format_trick(c)
}

/// One line: whitespace runs become one space, trimmed, at most `max`
/// characters.
pub fn clean_line(s: &str, max: usize) -> String {
    let joined = s.split_whitespace().collect::<Vec<_>>().join(" ");
    joined.chars().filter(|&c| keep(c)).take(max).collect::<String>().trim_end().to_string()
}

/// Several lines: each line trimmed at the end, tabs kept as spaces, at
/// most one blank line in a row, trimmed, at most `max` characters.
pub fn clean_text(s: &str, max: usize) -> String {
    let mut out = String::with_capacity(s.len().min(max * 4));
    let mut blank = 0;
    for line in s.lines() {
        let line: String = line.chars().map(|c| if c == '\t' { ' ' } else { c }).filter(|&c| keep(c)).collect();
        let line = line.trim_end();
        if line.trim().is_empty() {
            blank += 1;
            continue;
        }
        if !out.is_empty() {
            out.push_str(if blank > 0 { "\n\n" } else { "\n" });
        }
        blank = 0;
        out.push_str(line);
    }
    out.chars().take(max).collect::<String>().trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draft() -> Draft {
        Draft {
            kind: "bug".into(),
            priority: "high".into(),
            title: "  Poring   stuck\u{202E} ".into(),
            body: "It walks into the wall\r\n\r\n\r\n\tand never comes back out. \u{0007}\n".into(),
            client_version: "0.27".into(),
            user_agent: "x".repeat(500),
        }
    }

    #[test]
    fn validate_cleans_the_form() {
        let d = draft().validate().unwrap();
        assert_eq!(d.title, "Poring stuck");
        assert_eq!(d.body, "It walks into the wall\n\n and never comes back out.");
        assert_eq!(d.user_agent.len(), USER_AGENT_MAX);
    }

    #[test]
    fn validate_refuses_bad_fields() {
        assert_eq!(Draft { kind: "rant".into(), ..draft() }.validate(), Err(FeedbackError::BadKind));
        assert_eq!(Draft { priority: "urgent".into(), ..draft() }.validate(), Err(FeedbackError::BadPriority));
        assert_eq!(Draft { title: " a\u{200B}b ".into(), ..draft() }.validate(), Err(FeedbackError::TitleLength));
        assert_eq!(Draft { title: "t".repeat(TITLE_MAX + 1), ..draft() }.validate(), Err(FeedbackError::TitleLength));
        assert_eq!(Draft { body: "too short\n\n\n".into(), ..draft() }.validate(), Err(FeedbackError::BodyLength));
        assert_eq!(Draft { body: "b".repeat(BODY_MAX + 1), ..draft() }.validate(), Err(FeedbackError::BodyLength));
        assert!(Draft { body: "b".repeat(BODY_MAX), ..draft() }.validate().is_ok());
    }

    #[test]
    fn rate_limit_wants_a_gap_and_a_daily_cap() {
        assert_eq!(check_rate(0, None), Ok(()));
        assert_eq!(check_rate(3, Some(10)), Err(FeedbackError::TooSoon(MIN_GAP_SECS - 10)));
        assert_eq!(check_rate(3, Some(MIN_GAP_SECS)), Ok(()));
        assert_eq!(check_rate(MAX_PER_DAY, Some(3600)), Err(FeedbackError::DailyLimit));
    }
}

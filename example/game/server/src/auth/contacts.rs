//! Optional account contacts: an email address and a Telegram username.
//!
//! Players may give either at sign-up (`POST /api/auth/register`) or later
//! (`PATCH /api/auth/contacts`); nothing uses them yet. They are kept for
//! password recovery and notifications (SMTP, a Telegram bot) and for
//! Google sign-in (see example/docs/ROADMAP.md, Phase 6.27). Both are
//! unverified until those flows exist.
//!
//! What is stored is the normalised form, so the unique `lower(...)`
//! indexes compare like with like:
//! - email: trimmed, the domain lowercased, the local part as typed
//!   (strictly it is case-sensitive; uniqueness still ignores case, which
//!   is what every real provider does). An RFC 5321 "lite" check: one `@`,
//!   a dot-atom local part (no quoted strings, no comments), an ASCII or
//!   punycode host name with at least two labels and a non-numeric TLD,
//!   at most 254 characters.
//! - Telegram: trimmed, without a leading `@` or a `t.me/` /
//!   `telegram.me/` link prefix, matching Telegram's own rule
//!   `^[A-Za-z][A-Za-z0-9_]{4,31}$` (5 to 32 characters).
//!
//! An empty value means "not given".

use std::fmt;

/// Longest address SMTP can carry (RFC 5321 path minus the brackets).
pub const EMAIL_MAX: usize = 254;
const LOCAL_MAX: usize = 64;
const LABEL_MAX: usize = 63;
/// Telegram usernames: 5 to 32 characters.
pub const TELEGRAM_MIN: usize = 5;
pub const TELEGRAM_MAX: usize = 32;

/// Which contact a problem is about (the `field` of an error reply).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Contact {
    Email,
    Telegram,
}

impl Contact {
    pub fn field(self) -> &'static str {
        match self {
            Contact::Email => "email",
            Contact::Telegram => "telegram",
        }
    }

    /// `email_invalid` / `telegram_invalid`.
    pub fn invalid_code(self) -> &'static str {
        match self {
            Contact::Email => "email_invalid",
            Contact::Telegram => "telegram_invalid",
        }
    }

    /// `email_taken` / `telegram_taken`.
    pub fn taken_code(self) -> &'static str {
        match self {
            Contact::Email => "email_taken",
            Contact::Telegram => "telegram_taken",
        }
    }

    /// The unique index that guards it (`migrations/0002_contacts.sql`):
    /// a unique violation naming it means "taken" (a race the pre-check
    /// lost).
    pub fn index(self) -> &'static str {
        match self {
            Contact::Email => "users_email_lower_key",
            Contact::Telegram => "users_telegram_lower_key",
        }
    }
}

/// A rejected value: which contact and why (shown to the player).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Invalid {
    pub contact: Contact,
    pub reason: &'static str,
}

impl fmt::Display for Invalid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.reason)
    }
}

impl std::error::Error for Invalid {}

fn bad(contact: Contact, reason: &'static str) -> Invalid {
    Invalid { contact, reason }
}

/// `raw` as stored, `None` when empty. See the module notes for the rules.
pub fn normalize_email(raw: &str) -> Result<Option<String>, Invalid> {
    let bad = |reason| bad(Contact::Email, reason);
    let s = raw.trim();
    if s.is_empty() {
        return Ok(None);
    }
    if s.len() > EMAIL_MAX {
        return Err(bad("That email address is too long."));
    }
    let Some((local, domain)) = s.rsplit_once('@') else {
        return Err(bad("An email address looks like name@example.com."));
    };
    // Dot-atom local part: atext runs separated by single dots.
    const ATEXT_SPECIALS: &str = "!#$%&'*+/=?^_`{|}~-";
    let atom_ok = |a: &str| !a.is_empty() && a.chars().all(|c| c.is_ascii_alphanumeric() || ATEXT_SPECIALS.contains(c));
    if local.is_empty() || local.len() > LOCAL_MAX || !local.split('.').all(atom_ok) {
        return Err(bad("The part before @ is not a valid mailbox name."));
    }
    let domain = domain.to_ascii_lowercase();
    let label_ok = |l: &str| {
        !l.is_empty()
            && l.len() <= LABEL_MAX
            && !l.starts_with('-')
            && !l.ends_with('-')
            && l.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    };
    let labels: Vec<&str> = domain.split('.').collect();
    let tld_ok = labels.last().is_some_and(|t| t.len() >= 2 && !t.chars().all(|c| c.is_ascii_digit()));
    if labels.len() < 2 || !labels.iter().all(|l| label_ok(l)) || !tld_ok {
        return Err(bad("The part after @ is not a valid domain."));
    }
    Ok(Some(format!("{local}@{domain}")))
}

/// `raw` as stored, `None` when empty. Accepts `name`, `@name` and links
/// (`t.me/name`, `https://t.me/name`, `telegram.me/name`).
pub fn normalize_telegram(raw: &str) -> Result<Option<String>, Invalid> {
    // `get` (not slicing): a multi-byte character at the cut is no prefix.
    let strip = |s: &'_ str, prefixes: &[&str]| -> Option<usize> {
        prefixes.iter().find(|p| s.get(..p.len()).is_some_and(|h| h.eq_ignore_ascii_case(p))).map(|p| p.len())
    };
    let mut s = raw.trim();
    if let Some(n) = strip(s, &["https://", "http://"]) {
        s = &s[n..];
    }
    if let Some(n) = strip(s, &["www.t.me/", "t.me/", "www.telegram.me/", "telegram.me/"]) {
        s = &s[n..];
    }
    let s = s.strip_prefix('@').unwrap_or(s).trim_end_matches('/');
    if s.is_empty() {
        return Ok(None);
    }
    let mut chars = s.chars();
    let ok = (TELEGRAM_MIN..=TELEGRAM_MAX).contains(&s.len())
        && chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        && chars.all(|c| c.is_ascii_alphanumeric() || c == '_');
    if !ok {
        return Err(bad(
            Contact::Telegram,
            "A Telegram username has 5 to 32 letters, digits or _ and starts with a letter.",
        ));
    }
    Ok(Some(s.to_string()))
}

/// A consumer Google address (gmail.com, googlemail.com): the hint that
/// "Sign in with Google" will be offered for it. Google Workspace domains
/// cannot be told from the domain, and the address here is unverified
/// anyway: Google sign-in itself relies on the OAuth token's verified
/// email, never on this.
pub fn is_google_mail(email: &str) -> bool {
    email
        .rsplit_once('@')
        .is_some_and(|(_, d)| d.eq_ignore_ascii_case("gmail.com") || d.eq_ignore_ascii_case("googlemail.com"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emails_are_trimmed_and_their_domain_lowercased() {
        assert_eq!(normalize_email("  Ann.Lee+game@Example.COM ").unwrap().as_deref(), Some("Ann.Lee+game@example.com"));
        assert_eq!(normalize_email("a@b.co").unwrap().as_deref(), Some("a@b.co"));
        assert_eq!(normalize_email("x@mail.xn--p1ai").unwrap().as_deref(), Some("x@mail.xn--p1ai"));
        assert_eq!(normalize_email("o'neil_{1}@sub-domain.example.org").unwrap().as_deref(), Some("o'neil_{1}@sub-domain.example.org"));
        assert_eq!(normalize_email("").unwrap(), None);
        assert_eq!(normalize_email("   ").unwrap(), None);
    }

    #[test]
    fn malformed_emails_are_refused() {
        for raw in [
            "plain",
            "@example.com",
            "ann@",
            "ann@example",
            "ann@@example.com",
            "ann@exa mple.com",
            "an n@example.com",
            ".ann@example.com",
            "ann.@example.com",
            "a..b@example.com",
            "\"quoted\"@example.com",
            "ann@-example.com",
            "ann@example-.com",
            "ann@example..com",
            "ann@example.c",
            "ann@1.2.3.4",
            "ann@[127.0.0.1]",
            "ann@exämple.com",
            "änn@example.com",
        ] {
            let err = normalize_email(raw).unwrap_err();
            assert_eq!(err.contact, Contact::Email, "{raw:?}");
        }
        let long_local = format!("{}@example.com", "a".repeat(65));
        assert!(normalize_email(&long_local).is_err());
        let long = format!("{}@{}.com", "a".repeat(64), ["b".repeat(63), "c".repeat(63), "d".repeat(63)].join("."));
        assert!(long.len() > EMAIL_MAX);
        assert!(normalize_email(&long).is_err());
        let fits = format!("{}@{}.com", "a".repeat(60), ["b".repeat(63), "c".repeat(63), "d".repeat(61)].join("."));
        assert_eq!(fits.len(), EMAIL_MAX);
        assert!(normalize_email(&fits).unwrap().is_some());
    }

    #[test]
    fn telegram_names_lose_at_and_link_prefixes() {
        for raw in ["fox_dev", "@fox_dev", " @fox_dev ", "t.me/fox_dev", "https://t.me/fox_dev", "HTTP://T.ME/fox_dev/", "telegram.me/fox_dev", "https://www.t.me/@fox_dev"] {
            assert_eq!(normalize_telegram(raw).unwrap().as_deref(), Some("fox_dev"), "{raw:?}");
        }
        assert_eq!(normalize_telegram("PlugFox").unwrap().as_deref(), Some("PlugFox"), "case kept");
        assert_eq!(normalize_telegram("").unwrap(), None);
        assert_eq!(normalize_telegram("@").unwrap(), None);
        assert_eq!(normalize_telegram(&"a".repeat(32)).unwrap().map(|s| s.len()), Some(32));
    }

    #[test]
    fn malformed_telegram_names_are_refused() {
        for raw in ["abcd", "1abcde", "_abcde", "ab cde", "abc-de", "abcdé", "@@abcde", "t.me/joinchat/xyz12", &"a".repeat(33)] {
            let err = normalize_telegram(raw).unwrap_err();
            assert_eq!(err.contact, Contact::Telegram, "{raw:?}");
        }
    }

    #[test]
    fn google_mail_is_told_by_its_consumer_domains_only() {
        assert!(is_google_mail("ann@gmail.com"));
        assert!(is_google_mail("ann@GoogleMail.com"));
        assert!(!is_google_mail("ann@company.com"), "Workspace domains are not detectable");
        assert!(!is_google_mail("ann@gmail.com.evil.io"));
        assert!(!is_google_mail("gmail.com"));
    }
}

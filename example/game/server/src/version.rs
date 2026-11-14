//! The server's version: the package version plus the git commit it was
//! built from when known (`build.rs`), e.g. `0.1.0+1a2b3c4d`. Reported by
//! `/health`, the MCP `server_status` tool and the `connected` frame.

/// Package version (`Cargo.toml`).
pub const PACKAGE: &str = env!("CARGO_PKG_VERSION");
/// Short git hash of the build, when git was there.
pub const GIT_HASH: Option<&str> = option_env!("GAME_SERVER_GIT_HASH");

/// `PACKAGE`, then `+hash` when the commit is known.
pub fn version() -> &'static str {
    static FULL: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    FULL.get_or_init(|| match GIT_HASH {
        Some(h) if !h.is_empty() => format!("{PACKAGE}+{h}"),
        _ => PACKAGE.to_string(),
    })
}

#[cfg(test)]
mod tests {
    #[test]
    fn version_starts_with_the_package_version() {
        let v = super::version();
        assert!(v.starts_with(super::PACKAGE), "{v}");
        assert_eq!(v.contains('+'), super::GIT_HASH.is_some_and(|h| !h.is_empty()));
    }
}

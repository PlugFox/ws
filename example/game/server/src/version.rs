//! The server's version: the package version plus the git commit it was
//! built from when known (`build.rs`), e.g. `0.1.0+1a2b3c4d`. Reported by
//! `/health`, the MCP `server_status` tool and the `connected` frame.

/// Package version (`Cargo.toml`).
pub const PACKAGE: &str = env!("CARGO_PKG_VERSION");
/// Short git hash of the build, when git was there.
pub const GIT_HASH: Option<&str> = option_env!("GAME_SERVER_GIT_HASH");

/// `PACKAGE`, then `+hash` when the commit is known.
pub fn version() -> &'static str {
    static FULL: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| match GIT_HASH {
        Some(h) if !h.is_empty() => format!("{PACKAGE}+{h}"),
        _ => PACKAGE.to_string(),
    });
    &FULL
}

/// The client build this server accepts: the short git hash both are
/// stamped with (`build.rs` here, `GAME_BUILD` in the client's
/// `tool/build.sh`; the image passes the same `GIT_HASH` to both), `dev`
/// when built without git. Also in `GET /api/online` (`build`), so an open
/// tab notices a new deploy.
pub fn client_build() -> &'static str {
    match GIT_HASH {
        Some(h) if !h.is_empty() => h,
        _ => "dev",
    }
}

/// WebSocket close code: the client is another build, reload the page
/// (the reason is `update_required:<this server's build>`).
pub const CLOSE_UPDATE_REQUIRED: u16 = 4010;

/// How a connecting client's build (`?build=` on `/api/ws`) compares.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientBuild {
    /// The build this server accepts.
    Current,
    /// None given: a client from before the check, which knows no
    /// [`CLOSE_UPDATE_REQUIRED`] (it is told in a notification, then closed
    /// with the "kicked" code it does not retry).
    Missing,
    /// Another build.
    Other,
}

/// Compares a client's build with [`client_build`] (`expected`).
pub fn check_client_build(given: Option<&str>, expected: &str) -> ClientBuild {
    match given.map(str::trim) {
        None | Some("") => ClientBuild::Missing,
        Some(b) if b == expected => ClientBuild::Current,
        Some(_) => ClientBuild::Other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_build_check_accepts_only_the_same_build() {
        assert_eq!(check_client_build(Some("1a2b3c4d"), "1a2b3c4d"), ClientBuild::Current);
        assert_eq!(check_client_build(Some(" 1a2b3c4d "), "1a2b3c4d"), ClientBuild::Current);
        assert_eq!(check_client_build(Some("0f0f0f0f"), "1a2b3c4d"), ClientBuild::Other);
        assert_eq!(check_client_build(Some("dev"), "1a2b3c4d"), ClientBuild::Other);
        assert_eq!(check_client_build(None, "1a2b3c4d"), ClientBuild::Missing);
        assert_eq!(check_client_build(Some(""), "dev"), ClientBuild::Missing);
        assert_eq!(check_client_build(Some("dev"), "dev"), ClientBuild::Current);
    }

    #[test]
    fn client_build_is_the_git_hash_or_dev() {
        assert_eq!(client_build(), GIT_HASH.filter(|h| !h.is_empty()).unwrap_or("dev"));
        assert!(version().ends_with(client_build()) || client_build() == "dev");
    }

    #[test]
    fn version_starts_with_the_package_version() {
        let v = super::version();
        assert!(v.starts_with(super::PACKAGE), "{v}");
        assert_eq!(v.contains('+'), super::GIT_HASH.is_some_and(|h| !h.is_empty()));
    }
}

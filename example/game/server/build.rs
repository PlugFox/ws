//! Stamps the build with the git commit (`GAME_SERVER_GIT_HASH`, short
//! hash) when git and the repository are there; without them the version is
//! just the package version (`version::VERSION`).

use std::path::PathBuf;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    let s = String::from_utf8(out.stdout).ok()?.trim().to_string();
    (!s.is_empty()).then_some(s)
}

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    // `sqlx::migrate!` embeds migrations/: a new or edited file rebuilds.
    println!("cargo:rerun-if-changed=migrations");
    println!("cargo:rerun-if-env-changed=GAME_SERVER_GIT_HASH");
    // A hash given by the build environment (CI, Docker without .git) wins.
    if std::env::var("GAME_SERVER_GIT_HASH").is_ok() {
        return;
    }
    let Some(hash) = git(&["rev-parse", "--short=8", "HEAD"]) else { return };
    println!("cargo:rustc-env=GAME_SERVER_GIT_HASH={hash}");
    // Rebuild the stamp on a new commit or branch switch only (not on every
    // edit): HEAD and the branch ref it points at.
    // `--git-path` resolves both inside linked worktrees.
    let mut watch = vec!["HEAD".to_string()];
    if let Some(r) = git(&["symbolic-ref", "-q", "HEAD"]) {
        watch.push(r);
        watch.push("packed-refs".into());
    }
    for name in watch {
        if let Some(path) = git(&["rev-parse", "--path-format=absolute", "--git-path", &name]).map(PathBuf::from)
            && path.exists()
        {
            println!("cargo:rerun-if-changed={}", path.display());
        }
    }
}

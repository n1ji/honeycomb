//! "How many builds behind?" Compares the commit this binary was built from
//! with GitHub: with `main` through the git checkout it was built in, or, for
//! downloaded builds (no checkout on this computer), with the newest release
//! through GitHub's API.

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

const COMMIT: &str = env!("HONEYCOMB_COMMIT");
const REPO: &str = env!("HONEYCOMB_REPO");
const GITHUB: &str = env!("HONEYCOMB_GITHUB");
const BRANCH: &str = "main";
const CACHE: Duration = Duration::from_secs(600);

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Status {
    /// "upToDate", "behind", "ahead", "diverged" or "unknown".
    pub state: &'static str,
    pub behind: u32,
    pub ahead: u32,
    pub local: String,
    pub remote: String,
    pub repo: String,
    pub message: String,
    /// "checkout" (compared with `main` through git) or "release" (compared
    /// with GitHub's newest release).
    pub source: &'static str,
    /// The newest release's page, for downloading it.
    pub url: String,
}

static LAST: Mutex<Option<(Instant, Status)>> = Mutex::new(None);

/// Run git in the build checkout, never waiting on a password prompt.
fn git(args: &[&str], timeout: Duration) -> Option<String> {
    let mut child = Command::new("git")
        .arg("-C")
        .arg(REPO)
        .args(args)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_SSH_COMMAND", "ssh -o BatchMode=yes")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait().ok()? {
            Some(status) if status.success() => break,
            Some(_) => return None,
            None if start.elapsed() > timeout => {
                let _ = child.kill();
                return None;
            }
            None => std::thread::sleep(Duration::from_millis(50)),
        }
    }
    let out = child.wait_with_output().ok()?;
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn count(range: &str) -> Option<u32> {
    git(&["rev-list", "--count", range], Duration::from_secs(5))?.parse().ok()
}

fn blank(source: &'static str) -> Status {
    Status {
        state: "unknown",
        behind: 0,
        ahead: 0,
        local: COMMIT.to_string(),
        remote: String::new(),
        repo: REPO.to_string(),
        message: String::new(),
        source,
        url: String::new(),
    }
}

fn check() -> Status {
    // Only debug builds follow a checkout (release builds don't know one).
    if !cfg!(debug_assertions) || REPO.is_empty() || !std::path::Path::new(REPO).join(".git").exists() {
        return check_release();
    }
    let mut s = blank("checkout");
    let fetched = git(&["fetch", "--quiet", "origin", BRANCH], Duration::from_secs(20)).is_some();
    let remote_ref = format!("origin/{BRANCH}");
    s.remote = git(&["rev-parse", &remote_ref], Duration::from_secs(5)).unwrap_or_default();
    let (Some(behind), Some(ahead)) =
        (count(&format!("{COMMIT}..{remote_ref}")), count(&format!("{remote_ref}..{COMMIT}")))
    else {
        s.message = "Couldn't compare this build with GitHub.".into();
        return s;
    };
    s.behind = behind;
    s.ahead = ahead;
    s.state = match (behind, ahead) {
        (0, 0) => "upToDate",
        (_, 0) => "behind",
        (0, _) => "ahead",
        _ => "diverged",
    };
    s.message = match s.state {
        "upToDate" => "You're on the latest build.".into(),
        "behind" => format!("{behind} newer build{} on GitHub.", if behind == 1 { " is" } else { "s are" }),
        "ahead" => "This build has changes that aren't on GitHub yet.".into(),
        _ => format!("{behind} newer on GitHub, and {ahead} local change{} not pushed.", if ahead == 1 { "" } else { "s" }),
    };
    if !fetched {
        s.message.push_str(" (Couldn't reach GitHub just now, so this may be out of date.)");
    }
    s
}

#[derive(serde::Deserialize)]
struct Release {
    tag_name: String,
    name: Option<String>,
    html_url: String,
    draft: bool,
}

#[derive(serde::Deserialize)]
struct Compare {
    /// Commits the release has that this build doesn't.
    ahead_by: u32,
    /// Commits this build has that the release doesn't.
    behind_by: u32,
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout(Duration::from_secs(10))
        .user_agent(concat!("Honeycomb/", env!("CARGO_PKG_VERSION"), " (update check)"))
        .build()
}

/// A downloaded build: compare its commit with the newest published release
/// (pre-releases count; alphas are published as pre-releases).
fn check_release() -> Status {
    check_release_for(GITHUB, COMMIT)
}

/// `github` is "owner/name"; `commit` is this build's commit (or any ref).
fn check_release_for(github: &str, commit: &str) -> Status {
    let mut s = blank("release");
    s.local = commit.to_string();
    if commit.is_empty() {
        s.message = "This build doesn't know which commit it came from, so it can't check for updates.".into();
        return s;
    }
    let agent = agent();
    let api = format!("https://api.github.com/repos/{github}");
    let releases: Vec<Release> = match agent.get(&format!("{api}/releases?per_page=10")).call() {
        Ok(r) => match r.into_json() {
            Ok(v) => v,
            Err(_) => {
                s.message = "GitHub sent an answer Honeycomb couldn't read.".into();
                return s;
            }
        },
        Err(ureq::Error::Status(404, _)) => {
            s.message = "Couldn't find Honeycomb's releases on GitHub.".into();
            return s;
        }
        Err(ureq::Error::Status(403 | 429, _)) => {
            s.message = "GitHub is limiting update checks right now; try again later.".into();
            return s;
        }
        Err(_) => {
            s.message = "Couldn't reach GitHub to check for updates.".into();
            return s;
        }
    };
    let Some(latest) = releases.into_iter().find(|r| !r.draft) else {
        s.message = "No releases on GitHub yet.".into();
        return s;
    };
    s.remote = latest.tag_name.clone();
    // Only ever offer a download page on GitHub itself.
    if latest.html_url.starts_with("https://github.com/") {
        s.url = latest.html_url.clone();
    }
    let cmp: Compare = match agent.get(&format!("{api}/compare/{commit}...{}", latest.tag_name)).call() {
        Ok(r) => match r.into_json() {
            Ok(c) => c,
            Err(_) => {
                s.message = "GitHub sent an answer Honeycomb couldn't read.".into();
                return s;
            }
        },
        Err(_) => {
            s.message = "Couldn't compare this build with the newest release.".into();
            return s;
        }
    };
    s.behind = cmp.ahead_by;
    s.ahead = cmp.behind_by;
    s.state = match (s.behind, s.ahead) {
        (0, _) => "upToDate",
        _ => "behind",
    };
    let name = latest.name.filter(|n| !n.is_empty()).unwrap_or(latest.tag_name);
    s.message = if s.state == "upToDate" {
        "You're on the latest release.".into()
    } else {
        format!("{name} is available ({} newer change{}).", s.behind, if s.behind == 1 { "" } else { "s" })
    };
    s
}

/// Cached for 10 minutes unless `refresh` is set.
pub fn status(refresh: bool) -> Status {
    // HONEYCOMB_UPDATE_PREVIEW=3 pretends to be 3 builds behind, to see the lit bulb.
    if let Some(n) = std::env::var("HONEYCOMB_UPDATE_PREVIEW").ok().and_then(|v| v.parse::<u32>().ok()) {
        return Status {
            state: if n > 0 { "behind" } else { "upToDate" },
            behind: n,
            ahead: 0,
            local: COMMIT.to_string(),
            remote: String::new(),
            repo: REPO.to_string(),
            message: format!("Preview: pretending {n} newer builds are on GitHub."),
            source: "checkout",
            url: String::new(),
        };
    }
    if !refresh {
        if let Some((at, s)) = LAST.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
            if at.elapsed() < CACHE {
                return s.clone();
            }
        }
    }
    let s = check();
    *LAST.lock().unwrap_or_else(|p| p.into_inner()) = Some((Instant::now(), s.clone()));
    s
}

#[cfg(test)]
mod tests {
    /// Needs network and this checkout: cargo test version -- --ignored --nocapture
    #[test]
    #[ignore]
    fn compares_with_origin() {
        let s = super::status(true);
        println!("{s:?}");
        assert_ne!(s.state, "unknown", "{}", s.message);
    }

    /// Needs network. Runs the release check against a public repo with
    /// releases (Tauri's own): cargo test release_check -- --ignored --nocapture
    #[test]
    #[ignore]
    fn release_check() {
        let repo = "tauri-apps/tauri";
        let newest = super::check_release_for(repo, "HEAD");
        println!("{newest:?}");
        assert!(!newest.remote.is_empty() && newest.url.starts_with("https://github.com/"), "{}", newest.message);
        // The newest release compared with itself is up to date...
        let same = super::check_release_for(repo, &newest.remote);
        println!("{same:?}");
        assert_eq!(same.state, "upToDate", "{}", same.message);
        // ...and a years-old tag is far behind.
        let old = super::check_release_for(repo, "tauri-v2.0.0");
        println!("{old:?}");
        assert_eq!(old.state, "behind", "{}", old.message);
        assert!(old.behind > 100);
    }

    /// This repo while it's private: GitHub answers 404, which must read as a
    /// plain "unknown", not an error.
    #[test]
    #[ignore]
    fn release_check_on_this_repo() {
        let s = super::check_release();
        println!("{s:?}");
        assert_eq!(s.source, "release");
    }
}

//! "How many builds behind?" Compares the commit this binary was built from
//! with `main` on GitHub, using the git checkout it was built in.

use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;

const COMMIT: &str = env!("HONEYCOMB_COMMIT");
const REPO: &str = env!("HONEYCOMB_REPO");
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

fn check() -> Status {
    let mut s = Status {
        state: "unknown",
        behind: 0,
        ahead: 0,
        local: COMMIT.to_string(),
        remote: String::new(),
        repo: REPO.to_string(),
        message: String::new(),
    };
    if COMMIT.is_empty() || REPO.is_empty() || !std::path::Path::new(REPO).is_dir() {
        s.message = "This build wasn't made from a git checkout on this computer, so it can't check for updates.".into();
        return s;
    }
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
}

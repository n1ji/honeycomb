use std::path::Path;
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    // The commit and checkout this binary was built from, for the "builds behind" check.
    let commit = git(&["rev-parse", "HEAD"]).unwrap_or_default();
    // The checkout path is only baked into debug builds, which compare
    // themselves with it; release builds compare with GitHub's releases.
    let repo = if std::env::var("PROFILE").as_deref() == Ok("debug") {
        git(&["rev-parse", "--show-toplevel"]).unwrap_or_default()
    } else {
        String::new()
    };
    // "owner/name" of the GitHub repo, for builds that check releases instead of a checkout.
    let github = git(&["remote", "get-url", "origin"])
        .and_then(|url| {
            let rest = url.split("github.com").nth(1)?;
            let slug = rest.trim_start_matches([':', '/']).trim_end_matches('/').trim_end_matches(".git");
            (slug.split('/').count() == 2).then(|| slug.to_string())
        })
        .unwrap_or_else(|| "n1ji/honeycomb".into());
    println!("cargo:rustc-env=HONEYCOMB_COMMIT={commit}");
    println!("cargo:rustc-env=HONEYCOMB_REPO={repo}");
    println!("cargo:rustc-env=HONEYCOMB_GITHUB={github}");
    println!("cargo:rerun-if-env-changed=PROFILE");
    // Rebuild when HEAD moves (commit, pull, checkout).
    for p in ["../.git/HEAD", "../.git/refs/heads", "../.git/packed-refs", "../.git/config"] {
        if Path::new(p).exists() {
            println!("cargo:rerun-if-changed={p}");
        }
    }
    tauri_build::build()
}

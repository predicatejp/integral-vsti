// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use std::{env, fs, path::Path, process::Command};

fn git(args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(args)
        .output()
        .ok()?;
    output.status.success().then(|| {
        String::from_utf8(output.stdout)
            .expect("Git output must be UTF-8")
            .trim()
            .to_owned()
    })
}

fn main() {
    for variable in ["SAMPLE_VSTI_COMMIT", "SAMPLE_VSTI_RELEASE_VERSION"] {
        println!("cargo:rerun-if-env-changed={variable}");
    }
    for path in [
        "build.rs",
        "Cargo.toml",
        "Cargo.lock",
        "src",
        "tests",
        "scripts",
        ".cargo",
        ".github",
        "README.md",
        "AGENTS.md",
        "LICENSE",
        "COPYRIGHT",
        "THIRD_PARTY_NOTICES.txt",
        "deny.toml",
        ".gitignore",
    ] {
        if Path::new(path).exists() {
            println!("cargo:rerun-if-changed={path}");
        }
    }

    // 親ディレクトリの別リポジトリをソースアーカイブの出典と誤認しない。
    let in_repository = Path::new(".git").exists();
    let git_commit = in_repository.then(|| git(&["rev-parse", "HEAD"]).expect("Git HEAD"));
    if in_repository {
        // 通常clone、detached HEAD、worktree、packed refsのどれでもコミット更新を検知。
        let mut paths = vec![
            "HEAD".to_owned(),
            "index".to_owned(),
            "packed-refs".to_owned(),
        ];
        if let Some(reference) = git(&["symbolic-ref", "-q", "HEAD"]) {
            paths.push(reference);
        }
        for path in paths {
            let path = git(&["rev-parse", "--git-path", &path]).expect("Git metadata path");
            if Path::new(&path).exists() {
                println!("cargo:rerun-if-changed={path}");
            } else if let Some(parent) = Path::new(&path).parent().filter(|p| p.exists()) {
                // packed refからloose refが作られる場合も検知する。
                println!("cargo:rerun-if-changed={}", parent.display());
            }
        }
    }
    let explicit_commit = env::var("SAMPLE_VSTI_COMMIT").ok();
    if let (Some(explicit), Some(actual)) = (&explicit_commit, &git_commit) {
        assert_eq!(explicit, actual, "SAMPLE_VSTI_COMMIT must match Git HEAD");
    }
    let commit = git_commit.or(explicit_commit).expect(
        "Git checkout required; for a source archive, set SAMPLE_VSTI_COMMIT to its full commit ID",
    );
    assert!(
        commit.len() == 40 && commit.bytes().all(|b| b.is_ascii_hexdigit()),
        "Commit ID must contain exactly 40 hexadecimal characters"
    );
    let commit = commit.to_ascii_lowercase();
    let dirty = in_repository
        && !git(&["status", "--porcelain", "--untracked-files=normal"])
            .expect("Git status")
            .is_empty();
    let release = env::var("SAMPLE_VSTI_RELEASE_VERSION")
        .ok()
        .filter(|v| !v.is_empty())
        .map(|v| v.strip_prefix('v').unwrap_or(&v).to_owned());
    let version = if let Some(release) = &release {
        assert_eq!(
            release,
            &env::var("CARGO_PKG_VERSION").unwrap(),
            "Release version must match Cargo.toml"
        );
        assert!(!dirty, "Official releases require a clean working tree");
        release.clone()
    } else {
        format!("dev-{commit}")
    };
    assert!(
        version.len() < 64,
        "Version exceeds VST3 PClassInfo2 capacity"
    );
    println!("cargo:rustc-env=SAMPLE_VSTI_VERSION={version}");
    let target = env::var("TARGET").unwrap();
    let info = format!(
        "{{\n  \"version\": \"{version}\",\n  \"commit\": \"{commit}\",\n  \"dirty\": {dirty},\n  \"official_release\": {},\n  \"target\": \"{target}\"\n}}\n",
        release.is_some()
    );
    fs::write(
        Path::new(&env::var("OUT_DIR").unwrap()).join("build-info.json"),
        info,
    )
    .expect("Write build metadata");
}

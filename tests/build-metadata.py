#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Check real Cargo cache invalidation and release/development version selection."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent


def run(directory, *args, env=None, succeeds=True):
    result = subprocess.run(args, cwd=directory, env=env, text=True,
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    assert (result.returncode == 0) == succeeds, result.stdout + result.stderr
    return result.stdout.strip() if succeeds else result.stderr


with tempfile.TemporaryDirectory(prefix="integral-version-") as temporary:
    directory = Path(temporary)
    shutil.copy2(ROOT / "build.rs", directory / "build.rs")
    (directory / "Cargo.toml").write_text(
        '[package]\nname="version-fixture"\nversion="1.2.3"\nedition="2024"\n'
    )
    (directory / ".gitignore").write_text("/target/\n")
    (directory / "src").mkdir()
    source = 'fn main() { println!("{}", env!("INTEGRAL_VERSION")); }\n'
    (directory / "src/main.rs").write_text(source)
    environment = dict(os.environ)
    for key in ("INTEGRAL_COMMIT", "INTEGRAL_RELEASE_VERSION", "CARGO_TARGET_DIR"):
        environment.pop(key, None)
    run(directory, "cargo", "generate-lockfile", env=environment)
    run(directory, "git", "init", "-b", "main")
    run(directory, "git", "config", "user.name", "Version test")
    run(directory, "git", "config", "user.email", "version-test@example.invalid")
    run(directory, "git", "config", "commit.gpgsign", "false")
    run(directory, "git", "config", "core.autocrlf", "false")
    run(directory, "git", "add", ".")
    run(directory, "git", "commit", "-m", "Fixture")
    commit = run(directory, "git", "rev-parse", "HEAD")

    def cargo(env=environment, succeeds=True):
        return run(directory, "cargo", "run", "--quiet", "--locked", env=env, succeeds=succeeds)

    def metadata():
        paths = list((directory / "target/debug/build").glob("*/out/build-info.json"))
        assert len(paths) == 1
        return json.loads(paths[0].read_text())

    assert cargo() == f"dev-{commit}"
    assert metadata()["dirty"] is False
    assert cargo() == f"dev-{commit}"  # Cached build.
    run(directory, "git", "commit", "--allow-empty", "-m", "New revision")
    commit = run(directory, "git", "rev-parse", "HEAD")
    assert cargo() == f"dev-{commit}"  # No source change; Git ref must invalidate cache.
    assert metadata()["commit"] == commit

    run(directory, "git", "pack-refs", "--all")
    assert cargo() == f"dev-{commit}"
    run(directory, "git", "commit", "--allow-empty", "-m", "After packed refs")
    commit = run(directory, "git", "rev-parse", "HEAD")
    assert cargo() == f"dev-{commit}"
    run(directory, "git", "checkout", "--detach", "HEAD~1")
    commit = run(directory, "git", "rev-parse", "HEAD")
    assert cargo() == f"dev-{commit}"

    release_environment = dict(environment, INTEGRAL_RELEASE_VERSION="1.2.3")
    assert cargo(release_environment) == "1.2.3"
    assert metadata()["official_release"] is True
    assert cargo() == f"dev-{commit}"  # Switching back must invalidate cache too.
    assert "Release version must match" in cargo(
        dict(environment, INTEGRAL_RELEASE_VERSION="1.2.4"), succeeds=False
    )
    (directory / "src/main.rs").write_text(source + "// Local edit\n")
    assert cargo() == f"dev-{commit}"
    assert metadata()["dirty"] is True
    assert "clean working tree" in cargo(release_environment, succeeds=False)
    (directory / "src/main.rs").write_text(source)
    assert cargo(release_environment) == "1.2.3"
    assert "must match Git HEAD" in cargo(
        dict(environment, INTEGRAL_COMMIT="0" * 40), succeeds=False
    )

    # Source archives must declare their provenance instead of guessing a commit.
    (directory / ".git").rename(directory / "git-backup")
    assert "Git checkout required" in cargo(succeeds=False)
    assert cargo(dict(environment, INTEGRAL_COMMIT=commit)) == f"dev-{commit}"
    assert "40 hexadecimal" in cargo(
        dict(environment, INTEGRAL_COMMIT="invalid"), succeeds=False
    )

print("Build metadata checks passed: cached commits, dev/release, dirty tree, archive provenance")

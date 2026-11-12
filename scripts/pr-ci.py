#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Dispatch PR CI from the default branch, or validate its exact head revision."""

import argparse
import json
import os
import subprocess


def api(path, payload=None):
    command = ["gh", "api", path]
    if payload is not None:
        command += ["--method", "POST", "--input", "-"]
    result = subprocess.run(command, input=json.dumps(payload) if payload else None,
                            text=True, stdout=subprocess.PIPE, check=True)
    return json.loads(result.stdout) if result.stdout.strip() else None


def run(action):
    repository = os.environ["GITHUB_REPOSITORY"]
    number = os.environ.get("PR_NUMBER", "")
    if not number:
        ref = os.environ.get("GITHUB_REF", "")
        if action == "check" and (ref == "refs/heads/main" or ref.startswith("refs/tags/")):
            return
        raise ValueError("PR_NUMBER is required for a PR branch")
    if not number.isascii() or not number.isdecimal() or int(number) < 1:
        raise ValueError("PR_NUMBER must be a positive integer")
    if action == "dispatch":
        actor = os.environ["REQUESTER"]
        permission = api(f"repos/{repository}/collaborators/{actor}/permission")["permission"]
        if permission not in ("admin", "maintain", "write"):
            raise ValueError("Starting PR CI requires repository write permission")
    pr = api(f"repos/{repository}/pulls/{number}")
    if pr["state"] != "open" or pr["base"]["ref"] != "main":
        raise ValueError("CI requires an open pull request targeting main")
    head = pr["head"]
    if not head["repo"] or head["repo"]["full_name"] != repository:
        raise ValueError("Dispatch requires a head branch in this repository")
    if action == "check":
        if os.environ["GITHUB_SHA"] != head["sha"] or os.environ["GITHUB_REF"] != "refs/heads/" + head["ref"]:
            raise ValueError("Select the current PR head branch and commit")
    else:
        api(f"repos/{repository}/actions/workflows/rust.yml/dispatches",
            {"ref": head["ref"], "inputs": {"pr_number": number}})
    print(f"PR #{number}: {head['ref']} at {head['sha']}")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("dispatch", "check"))
    run(parser.parse_args().action)

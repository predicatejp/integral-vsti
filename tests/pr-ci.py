#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Guard manual CI dispatch permissions and checks for the exact PR revision."""

import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import unittest
from unittest.mock import Mock, call, patch

spec = importlib.util.spec_from_file_location("pr_ci", Path(__file__).parents[1] / "scripts/pr-ci.py")
ci = importlib.util.module_from_spec(spec)
spec.loader.exec_module(ci)

REPO = "owner/integral-vsti"
HEAD = "a" * 40
PR = {"state": "open", "base": {"ref": "main"},
      "head": {"repo": {"full_name": REPO}, "ref": "feature/example", "sha": HEAD}}


class ManualCiTests(unittest.TestCase):
    def setUp(self):
        self.env = patch.dict(os.environ, {
            "GITHUB_REPOSITORY": REPO, "PR_NUMBER": "12", "REQUESTER": "reviewer",
            "GITHUB_REF": "refs/heads/feature/example", "GITHUB_SHA": HEAD,
        })
        self.env.start()
        self.addCleanup(self.env.stop)

    def test_writer_dispatches_to_pr_head(self):
        for permission in ("admin", "maintain", "write"):
            with self.subTest(permission=permission), patch.object(ci, "api", side_effect=[
                {"permission": permission}, PR, None,
            ]) as api:
                ci.run("dispatch")
                self.assertEqual(api.call_args_list, [
                    call(f"repos/{REPO}/collaborators/reviewer/permission"),
                    call(f"repos/{REPO}/pulls/12"),
                    call(f"repos/{REPO}/actions/workflows/rust.yml/dispatches",
                         {"ref": "feature/example", "inputs": {"pr_number": "12"}}),
                ])

    def test_non_writer_cannot_dispatch(self):
        for permission in ("read", "triage", "none"):
            with self.subTest(permission=permission), patch.object(ci, "api", return_value={
                "permission": permission,
            }) as api, self.assertRaises(ValueError):
                ci.run("dispatch")
            self.assertEqual(api.call_count, 1)

    def test_invalid_pr_never_dispatches(self):
        changes = [({"state": "closed"}), ({"base": {"ref": "develop"}})]
        for repo in (None, {"full_name": "fork/integral-vsti"}):
            changes.append({"head": {**PR["head"], "repo": repo}})
        for change in changes:
            pr = {**copy.deepcopy(PR), **change}
            with self.subTest(change=change), patch.object(ci, "api", side_effect=[
                {"permission": "write"}, pr,
            ]) as api, self.assertRaises(ValueError):
                ci.run("dispatch")
            self.assertEqual(api.call_count, 2)

    def test_invalid_number_is_rejected_before_any_api_call(self):
        for number in ("0", "-1", "１", "12/merge", "12;echo unsafe"):
            with self.subTest(number=number), patch.dict(os.environ, {"PR_NUMBER": number}), \
                 patch.object(ci, "api") as api, self.assertRaises(ValueError):
                ci.run("dispatch")
            api.assert_not_called()

    def test_target_accepts_only_current_head_branch_and_commit(self):
        with patch.object(ci, "api", return_value=PR) as api:
            ci.run("check")
            api.assert_called_once_with(f"repos/{REPO}/pulls/12")
        for mismatch in ({"GITHUB_SHA": "b" * 40}, {"GITHUB_REF": "refs/heads/main"},
                         {"GITHUB_REF": "refs/tags/v0.1.0"}):
            with self.subTest(mismatch=mismatch), patch.dict(os.environ, mismatch), \
                 patch.object(ci, "api", return_value=PR), self.assertRaises(ValueError):
                ci.run("check")

    def test_missing_pr_number_only_allows_main_or_tag_check(self):
        with patch.dict(os.environ, {"PR_NUMBER": ""}), patch.object(ci, "api") as api:
            for ref in ("refs/heads/main", "refs/tags/v0.1.0"):
                with patch.dict(os.environ, {"GITHUB_REF": ref}):
                    ci.run("check")
                    with self.assertRaises(ValueError):
                        ci.run("dispatch")
            with self.assertRaises(ValueError):
                ci.run("check")
            api.assert_not_called()

    def test_lookup_failure_cannot_start_ci(self):
        error = subprocess.CalledProcessError(1, ["gh", "api"])
        with patch.object(ci, "api", side_effect=error) as api, \
             self.assertRaises(subprocess.CalledProcessError):
            ci.run("dispatch")
        self.assertEqual(api.call_count, 1)

    def test_api_sends_branch_as_json_without_shell_evaluation(self):
        payload = {"ref": "feature/$(exit)", "inputs": {"pr_number": "12"}}
        with patch.object(subprocess, "run", return_value=Mock(stdout="")) as run:
            ci.api("repos/owner/integral-vsti/actions/workflows/rust.yml/dispatches", payload)
        self.assertEqual(run.call_args.args[0][-4:], ["--method", "POST", "--input", "-"])
        self.assertEqual(json.loads(run.call_args.kwargs["input"]), payload)
        self.assertNotIn("shell", run.call_args.kwargs)
        self.assertTrue(run.call_args.kwargs["check"])


if __name__ == "__main__":
    unittest.main()

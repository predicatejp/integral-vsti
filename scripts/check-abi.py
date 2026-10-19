#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build and invoke the SDK-based ABI test with the expected embedded version."""

import importlib.util
import os
from pathlib import Path
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("bundle", ROOT / "scripts/build-vst3.py")
bundle_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle_module)

sdk = os.environ.get("VST3_SDK_DIR")
if not sdk:
    raise SystemExit("Set VST3_SDK_DIR to the official VST3 SDK checkout")
_, binary, info = bundle_module.build()
source = str(ROOT / "tests/abi-smoke.cpp")
if sys.platform == "win32":
    test = ROOT / "target/abi-smoke.exe"
    command = ["cl", "/nologo", "/std:c++17", "/EHsc", "/W4", "/WX", "/utf-8",
               "/UNDEBUG", "/DNOMINMAX", f"/I{sdk}", source,
               f"/Fo{ROOT / 'target/abi-smoke.obj'}", f"/Fe{test}"]
else:
    test = ROOT / "target/abi-smoke"
    command = [os.environ.get("CXX", "c++"), "-std=c++17", "-Wall", "-Wextra",
               "-Werror", "-UNDEBUG", "-I", sdk, source, "-ldl", "-o", str(test)]
subprocess.run(command, cwd=ROOT, check=True)
normal_environment = dict(os.environ)
normal_environment.pop("SAMPLE_VSTI_DIAGNOSTICS_DIR", None)
subprocess.run([str(test), str(binary), info["version"]], cwd=ROOT,
               env=normal_environment, check=True)

# 診断の有効・無効を別プロセスで検証する。実際のSDK呼び出しとレポートの対応を確認。
with tempfile.TemporaryDirectory(prefix="sample-vsti-diagnostics-") as directory:
    environment = dict(os.environ, SAMPLE_VSTI_DIAGNOSTICS_DIR=directory)
    subprocess.run([str(test), str(binary), info["version"]], cwd=ROOT,
                   env=environment, check=True)
    reports = []
    for path in Path(directory).glob("*.txt"):
        report = dict(line.split("=", 1) for line in path.read_text().splitlines())
        assert report["version"] == info["version"]
        assert report["finished"] == "true"
        assert int(report["process_calls"]) > 0
        reports.append(report)
    assert len(reports) == 100, len(reports)
    assert any(int(report["nonzero_output_blocks"]) > 0 for report in reports)
    assert any(int(report["note_on_events"]) == 0 for report in reports)
    assert any(int(report["note_on_rejected"]) > 0 for report in reports)
    assert any(int(report["event_read_failures"]) > 0 for report in reports)
    print("Verified opt-in diagnostics: 100 reports, audible/silent instances and rejected events")

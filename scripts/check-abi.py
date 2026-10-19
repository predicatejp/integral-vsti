#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build and invoke the SDK-based ABI test with the expected embedded version."""

import importlib.util
import os
from pathlib import Path
import subprocess
import sys

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
subprocess.run([str(test), str(binary), info["version"]], cwd=ROOT, check=True)

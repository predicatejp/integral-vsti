#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build a native bundle and ZIP using Cargo's exact build metadata."""

import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def build():
    if platform.machine().lower() not in ("x86_64", "amd64"):
        raise RuntimeError("Only x86_64 hosts are supported")
    if sys.platform == "win32":
        target, platform_name, binary_name = (
            "x86_64-pc-windows-msvc", "windows-x86_64", "SampleVsti.vst3"
        )
        architecture = "x86_64-win"
    elif sys.platform == "linux":
        target, platform_name, binary_name = (
            "x86_64-unknown-linux-gnu", "linux-x86_64", "SampleVsti.so"
        )
        architecture = "x86_64-linux"
    else:
        raise RuntimeError("Only Linux and Windows hosts are supported")

    result = subprocess.run(
        ["cargo", "build", "--release", "--locked", "--target", target,
         "--target-dir", str(ROOT / "target"), "--message-format=json-render-diagnostics"],
        cwd=ROOT, text=True, encoding="utf-8", stdout=subprocess.PIPE, check=True,
    )
    binary = info_path = None
    for line in result.stdout.splitlines():
        message = json.loads(line)
        if message["reason"] == "build-script-executed":
            candidate = Path(message["out_dir"]) / "build-info.json"
            if candidate.is_file():
                info_path = candidate
        elif (message["reason"] == "compiler-artifact"
              and message["target"]["name"] == "sample_vsti"
              and "cdylib" in message["target"]["crate_types"]):
            binary = next(Path(p) for p in message["filenames"]
                          if Path(p).suffix in (".dll", ".so"))
    if binary is None or info_path is None:
        raise RuntimeError("Cargo did not report the binary and build metadata")
    info = json.loads(info_path.read_text(encoding="utf-8"))
    if info["target"] != target:
        raise RuntimeError("Build metadata target mismatch")
    if os.environ.get("GITHUB_ACTIONS") == "true" and info["dirty"]:
        raise RuntimeError("CI bundles must be built from a clean checkout")

    bundle = ROOT / "target/vst3/SampleVsti.vst3"
    if bundle.exists():
        shutil.rmtree(bundle)
    binary_dir = bundle / "Contents" / architecture
    licenses = bundle / "Contents/Resources/Licenses"
    binary_dir.mkdir(parents=True)
    licenses.mkdir(parents=True)
    shutil.copy2(binary, binary_dir / binary_name)
    for name in ("LICENSE", "COPYRIGHT", "THIRD_PARTY_NOTICES.txt"):
        shutil.copy2(ROOT / name, licenses / name)
    shutil.copy2(info_path, bundle / "Contents/Resources/build-info.json")
    if sys.platform == "win32":
        shutil.copy2(ROOT / "scripts/collect-diagnostics.ps1",
                     bundle / "Contents/Resources/collect-diagnostics.ps1")

    pr = os.environ.get("PR_NUMBER", "")
    if pr and not pr.isdecimal():
        raise RuntimeError("PR_NUMBER must be numeric")
    identity = f"pr{pr}-{info['commit']}" if pr else info["version"]
    name = f"SampleVsti-{platform_name}-{identity}"
    archive = ROOT / "target/dist" / f"{name}.zip"
    archive.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(archive, "w", compression=zipfile.ZIP_DEFLATED) as output:
        for path in sorted(bundle.rglob("*")):
            if path.is_file():
                output.write(path, path.relative_to(bundle.parent).as_posix())
    print(f"Version: {info['version']} (dirty={info['dirty']})")
    print(f"Bundle: {bundle}")
    print(f"ZIP: {archive}")
    if os.environ.get("GITHUB_OUTPUT"):
        with open(os.environ["GITHUB_OUTPUT"], "a", encoding="utf-8") as output:
            output.write(f"artifact_name={name}\nversion={info['version']}\n")
            output.write(f"commit={info['commit']}\n")
    return bundle, binary_dir / binary_name, info


if __name__ == "__main__":
    build()

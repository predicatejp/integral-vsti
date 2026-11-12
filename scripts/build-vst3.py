#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build a VST3 bundle and ZIP using Cargo's exact build metadata."""

import argparse
import importlib.util
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def build(target=None):
    if platform.machine().lower() not in ("x86_64", "amd64"):
        raise RuntimeError("Only x86_64 hosts are supported")
    native_target = {
        "win32": "x86_64-pc-windows-msvc",
        "linux": "x86_64-unknown-linux-gnu",
    }.get(sys.platform)
    if native_target is None:
        raise RuntimeError("Only Linux and Windows hosts are supported")
    target = target or native_target
    cargo = ["cargo"]
    if target != native_target:
        if sys.platform != "linux" or target != "x86_64-pc-windows-msvc":
            raise RuntimeError("Cross builds support Linux to Windows MSVC only")
        cargo.append("xwin")
    if target == "x86_64-pc-windows-msvc":
        platform_name, binary_name = "windows-x86_64", "Integral.vst3"
        architecture = "x86_64-win"
    else:
        platform_name, binary_name = "linux-x86_64", "Integral.so"
        architecture = "x86_64-linux"

    result = subprocess.run(
        cargo + ["build", "--release", "--locked", "--target", target,
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
              and message["target"]["name"] == "integral"
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

    bundle = ROOT / "target/vst3/Integral.vst3"
    if bundle.exists():
        shutil.rmtree(bundle)
    binary_dir = bundle / "Contents" / architecture
    licenses = bundle / "Contents/Resources/Licenses"
    binary_dir.mkdir(parents=True)
    licenses.mkdir(parents=True)
    shutil.copy2(binary, binary_dir / binary_name)
    for name in ("LICENSE", "COPYRIGHT", "THIRD_PARTY_NOTICES.txt"):
        shutil.copy2(ROOT / name, licenses / name)
    spec = importlib.util.spec_from_file_location("licenses", ROOT / "scripts/bundle-licenses.py")
    license_module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(license_module)
    license_module.collect(licenses / "dependencies", target)
    shutil.copy2(info_path, bundle / "Contents/Resources/build-info.json")

    pr = os.environ.get("PR_NUMBER", "")
    if pr and not pr.isdecimal():
        raise RuntimeError("PR_NUMBER must be numeric")
    identity = f"pr{pr}-{info['commit']}" if pr else info["version"]
    name = f"Integral-{platform_name}-{identity}"
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
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=("x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc"),
                        help="Build target (default: native host)")
    build(parser.parse_args().target)

#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Collect the locked target's crate and bundled-font notices without source code."""

import json
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parent.parent
# These published crates omit their repository-level license files.
FALLBACKS = {
    **{(name, "0.37.0"): "egui" for name in
       ("egui", "ecolor", "emath", "epaint", "egui_glow")},
    ("clipboard-win", "5.4.1"): "clipboard-win",
    ("accesskit", "0.25.1"): "accesskit",
    ("profiling", "1.0.18"): "profiling",
}


def collect(destination, target):
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--filter-platform", target],
        cwd=ROOT, text=True, encoding="utf-8",
    ))
    index = []
    for package in sorted(metadata["packages"], key=lambda p: (p["name"], p["version"])):
        if package["source"] is None:
            continue
        name, version = package["name"], package["version"]
        root = Path(package["manifest_path"]).parent
        output = destination / f"{name}-{version}"
        files = [p for p in root.rglob("*") if p.is_file() and (
            any(word in p.name.lower() for word in ("license", "copying", "copyright", "notice"))
            or ("fonts" in p.relative_to(root).parts and p.suffix.lower() == ".txt")
        ) and p.suffix.lower() not in (".rs", ".html", ".py")]
        if files:
            for path in files:
                dest = output / path.relative_to(root)
                dest.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(path, dest)
        else:
            fallback = FALLBACKS.get((name, version))
            if fallback is None:
                raise RuntimeError(f"Missing license text for {name} {version}")
            shutil.copytree(ROOT / "licenses/upstream" / fallback, output)
        index.append({"name": name, "version": version, "license": package["license"],
                      "source": package["source"], "repository": package["repository"],
                      "notices": output.name})
    (destination / "index.json").write_text(
        json.dumps(index, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

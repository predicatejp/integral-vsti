#!/usr/bin/env python3
# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only

"""Build and invoke the SDK-based ABI test with the expected embedded version."""

import argparse
import importlib.util
import os
from pathlib import Path
import subprocess
import struct
import zlib
import sys

ROOT = Path(__file__).resolve().parent.parent
spec = importlib.util.spec_from_file_location("bundle", ROOT / "scripts/build-vst3.py")
bundle_module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(bundle_module)

parser = argparse.ArgumentParser()
parser.add_argument("--gui", action="store_true", help="Test the embedded GUI on a desktop/Xvfb")
options = parser.parse_args()
extra_args = ["--gui"] if options.gui else []

sdk = os.environ.get("VST3_SDK_DIR")
if not sdk:
    raise SystemExit("Set VST3_SDK_DIR to the official VST3 SDK checkout")
_, binary, info = bundle_module.build()
source = str(ROOT / "tests/abi-smoke.cpp")
if sys.platform == "win32":
    test = ROOT / "target/abi-smoke.exe"
    command = ["cl", "/nologo", "/std:c++17", "/EHsc", "/W4", "/WX", "/utf-8",
               "/UNDEBUG", "/DNOMINMAX", f"/I{sdk}", source,
               f"/Fo{ROOT / 'target/abi-smoke.obj'}", f"/Fe{test}", "user32.lib", "gdi32.lib", "opengl32.lib"]
else:
    test = ROOT / "target/abi-smoke"
    command = [os.environ.get("CXX", "c++"), "-std=c++17", "-Wall", "-Wextra",
               "-Werror", "-UNDEBUG", "-I", sdk, source, "-ldl", "-lX11", "-o", str(test)]
subprocess.run(command, cwd=ROOT, check=True)
subprocess.run([str(test), str(binary), info["version"], *extra_args], cwd=ROOT, check=True)

if options.gui:
    # 検証ホストが取得した実ウィンドウのRGB画像を、追加依存なしでPNGへ保存する。
    def chunk(kind, data):
        return (struct.pack(">I", len(data)) + kind + data
                + struct.pack(">I", zlib.crc32(kind + data)))

    for source_name, name in [("gui-panel", "gui-panel"), ("gui-choices", "gui-choices"),
                              ("gui-smoke", "gui-overlay"), ("gui-voices", "gui-voices"),
                              ("gui-delay", "gui-delay"), ("gui-delay-time", "gui-delay-time"), ("gui-reverb", "gui-reverb")]:
        with (ROOT / "target" / f"{source_name}.ppm").open("rb") as image:
            assert image.readline() == b"P6\n"
            width, height = map(int, image.readline().split())
            assert image.readline() == b"255\n"
            rgb = image.read()
        assert len(rgb) == width * height * 3
        rows = b"".join(b"\0" + rgb[y * width * 3:(y + 1) * width * 3]
                        for y in range(height))
        png = (b"\x89PNG\r\n\x1a\n"
               + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 2, 0, 0, 0))
               + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b""))
        path = ROOT / "target" / f"{name}.png"
        path.write_bytes(png)
        print(f"Rendered image: {path}")

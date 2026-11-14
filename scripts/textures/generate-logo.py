# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""SVGの輪郭を使い、金属面に彫り込んだロゴを生成する。"""

from pathlib import Path
import struct
import subprocess
import xml.etree.ElementTree as ET
import zlib

WIDTH = 256
HEIGHT = 60
LOGO_WIDTH = 198
LOGO_HEIGHT = 42
ROOT = Path(__file__).resolve().parents[2]
OUTPUT = ROOT / "src/editor/logo-engraved"


def pixels():
    ET.register_namespace("", "http://www.w3.org/2000/svg")
    svg = ET.parse(ROOT / "artwork/integral-logo-T1-black.svg").getroot()
    _, _, width, height = map(float, svg.attrib["viewBox"].split())
    svg.set("width", str(LOGO_WIDTH * 4))
    svg.set("height", str(LOGO_WIDTH * 4 * height / width))
    # SVGを高解像度で描画し、輪郭を維持したまま一度だけ縮小する。
    mask = subprocess.run(
        ["convert", "-background", "none", "svg:-", "-resize",
         f"{LOGO_WIDTH}x{LOGO_HEIGHT}", "-gravity", "center", "-extent",
         f"{WIDTH}x{HEIGHT}", "-alpha", "extract", "-depth", "8", "gray:-"],
        input=ET.tostring(svg), check=True, capture_output=True,
    ).stdout
    assert len(mask) == WIDTH * HEIGHT
    output = bytearray()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            value = mask[y * WIDTH + x]
            shifted = mask[(y - 1) * WIDTH + x - 1] if x and y else 0
            # 上左の内壁を暗く、右下の外縁を明るくして浅い彫込みを表す。
            layers = (
                ((25, 39, 50), value * 65),
                ((10, 17, 23), max(value - shifted, 0) * 110),
                ((255, 255, 255), max(shifted - value, 0) * 140),
            )
            color = [0.0, 0.0, 0.0]
            alpha = 0.0
            for rgb, opacity in layers:
                opacity /= 255 * 255
                color = [c * opacity + p * (1.0 - opacity)
                         for c, p in zip(rgb, color)]
                alpha = opacity + alpha * (1.0 - opacity)
            output.extend(round(c / alpha) if alpha else 0 for c in color)
            output.append(round(alpha * 255))
    return bytes(output)


def png(data):
    def chunk(kind, payload):
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload)))

    rows = b"".join(b"\0" + data[y * WIDTH * 4:(y + 1) * WIDTH * 4]
                    for y in range(HEIGHT))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    data = pixels()
    OUTPUT.with_suffix(".rgba").write_bytes(data)
    OUTPUT.with_suffix(".png").write_bytes(png(data))

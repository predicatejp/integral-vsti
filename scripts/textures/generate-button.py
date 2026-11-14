# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""黒いラバー製ボタンの丸面取りと落ち影を9分割伸縮用のRGBA画像へ生成する。"""

import math
from pathlib import Path
import struct
import zlib

WIDTH, HEIGHT = 96, 40
SAMPLES = 4
OUTPUT = Path(__file__).resolve().parents[2] / "src/editor/button-rubber"


def outline(x, y):
    # 本体は(6,6)〜(90,34)、角半径4px。固定12pxの9分割領域内で角を完結させる。
    qx, qy = abs(x - 48) - 38, abs(y - 20) - 10
    ox, oy = max(qx, 0.0), max(qy, 0.0)
    length = math.hypot(ox, oy)
    ny = oy / length if length else float(qy > qx)
    return min(max(qx, qy), 0.0) + length - 4, math.copysign(ny, y - 20)


def surface(x, y):
    distance, ny = outline(x, y)
    if distance > 0:
        # 上方の面光源を有限幅で積分し、落ち影を下側へ限定する。
        coverage = 0.0
        for i in range(16):
            t = (i + 0.5) / 8 - 1
            d, _ = outline(x, y - (2.7 + 1.2 * t))
            coverage += max(0.0, min(1.0, 0.5 - d)) * (1 - t * t)
        contact = 0.18 * math.exp(-distance / 0.3)
        return (7.0, 10.0, 12.0, contact + 0.48 * coverage / 10.6875)

    # 3.1pxの丸面取り。拡散反射が主体のラバーに、ごく弱い広い反射を重ねる。
    tilt = max(0.0, 1 + distance / 3.1) * math.pi / 2
    nr, nz = math.sin(tilt), math.cos(tilt)
    diffuse = max(0.0, nz * math.cos(math.radians(70))
                  - ny * nr * math.sin(math.radians(70)))
    specular = max(0.0, nz * math.cos(math.radians(35))
                   - ny * nr * math.sin(math.radians(35))) ** 12
    level = 24.0 + 45.0 * diffuse + 6.0 * specular
    # 微細な粒は1階調未満に抑える。横方向の勾配や大きい模様を作らず伸縮に備える。
    grain = ((math.sin(math.floor(x) * 17.13 + math.floor(y) * 71.7)
              * 43758.5453) % 1 - 0.5) * 1.5
    level += grain * min(1.0, -distance / 3.1)
    edge = min(1.0, -distance / 0.3)
    level = 9.0 + (level - 9.0) * edge
    return (level, level + 1.0, level + 2.0, 1.0)


def pixels():
    output = bytearray()
    for y in range(HEIGHT):
        for x in range(WIDTH):
            accumulated = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    rgba = surface(x + (sx + 0.5) / SAMPLES,
                                   y + (sy + 0.5) / SAMPLES)
                    for channel in range(3):
                        accumulated[channel] += rgba[channel] * rgba[3]
                    accumulated[3] += rgba[3]
            alpha = accumulated[3]
            output.extend(round(v / alpha) if alpha else 0 for v in accumulated[:3])
            output.append(round(alpha * 255 / SAMPLES ** 2))
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

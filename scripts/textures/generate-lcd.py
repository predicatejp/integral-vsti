# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""LCD選択欄のベゼル・表示面・落ち影を9分割伸縮用のRGBA画像へ生成する。"""

import math
from pathlib import Path
import struct
import zlib

WIDTH, HEIGHT = 96, 40
SAMPLES = 4
OUTPUT = Path(__file__).resolve().parents[2] / "src/editor/lcd-display"


def outline(x, y):
    # 本体は(6,6)〜(90,34)、角半径4px。距離と外向き法線のy成分を返す。
    qx, qy = abs(x - 48) - 38, abs(y - 20) - 10
    ox, oy = max(qx, 0.0), max(qy, 0.0)
    length = math.hypot(ox, oy)
    ny = oy / length if length else float(qy > qx)
    return min(max(qx, qy), 0.0) + length - 4, math.copysign(ny, y - 20)


def surface(x, y):
    distance, ny = outline(x, y)
    if distance > 0:
        # 上方の面光源で、台座の影は下方へ落とす。有限幅の半影とする。
        coverage = 0.0
        for i in range(16):
            t = (i + 0.5) / 8 - 1
            d, _ = outline(x, y - (3.0 + 1.5 * t))
            coverage += max(0.0, min(1.0, 0.5 - d)) * (1 - t * t)
        contact = 0.30 * math.exp(-distance / 0.7)
        alpha = contact + 0.52 * coverage / 10.6875
        return (8.0, 12.0, 10.0, alpha)
    if distance > -0.35:
        return (21.0, 25.0, 23.0, 1.0)
    if distance > -1.1:
        # 張り出した外側の面取りは上縁で鋭く反射し、下側は側面の暗さを出す。
        highlight = 25.0 * max(-ny, 0.0) * math.exp(-((distance + 0.65) / 0.25) ** 2)
        level = min(253.0, 150.0 - 90.0 * ny + highlight)
        return (level, level + 2.0, level + 1.0, 1.0)
    if distance > -2.1:
        # ベゼル天面の幅を確保し、外側面と表示面の内壁を分離する。
        level = 114.0 - 25.0 * ny
        return (level, level + 3.0, level + 1.0, 1.0)
    if distance > -3.1:
        # 表示面へ降りる内壁は上が暗く、下縁が照らされる。
        level = max(7.0, 25.0 + 45.0 * ny)
        return (level, level + 7.0, level + 2.0, 1.0)
    # 黒緑のバックライトと上方の環境が映るガラス面。反射は横方向に一定とする。
    glow = math.exp(-((y - 22.0) / 8.0) ** 2)
    edge = max(0.0, min(1.0, (-distance - 3.1) / 2.9))
    inset = 0.3 + 0.7 * edge * edge * (3.0 - 2.0 * edge)
    reflection = 7.0 * math.exp(-((y - 13.0) / 4.0) ** 2) * inset
    return (12.0 + 4.0 * glow * inset + reflection,
            21.0 + 17.0 * glow * inset + reflection,
            16.0 + 8.0 * glow * inset + reflection, 1.0)


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

# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""1076×768のヘアライン金属板を生成する。通常ビルド時は実行しない。"""

from pathlib import Path
import random
import struct
import zlib

WIDTH = 1076
HEIGHT = 768
SOURCE_WIDTH = WIDTH // 2
OUTPUT = Path(__file__).resolve().parents[2] / "src/editor/panel-metal"


def pixels():
    random_source = random.Random(0x494E54454752414C)
    noise = [random_source.random() - 0.5 for _ in range(SOURCE_WIDTH * HEIGHT)]

    def brushed(width):
        # 両端を含め全画素に同じ循環畳み込みを適用し、継ぎ目を作らない。
        half = width // 2
        return [
            sum(noise[y * SOURCE_WIDTH + (x + dx) % SOURCE_WIDTH]
                for dx in range(-half, half + 1)) / width
            for y in range(HEIGHT) for x in range(SOURCE_WIDTH)
        ]

    long = brushed(31)
    short = brushed(7)
    grain = [1.35 * (37.0 * a + 8.0 * b) for a, b in zip(long, short)]
    grain = [
        0.76 * grain[y * SOURCE_WIDTH + x]
        + 0.12 * grain[((y - 1) % HEIGHT) * SOURCE_WIDTH + x]
        + 0.12 * grain[((y + 1) % HEIGHT) * SOURCE_WIDTH + x]
        for y in range(HEIGHT) for x in range(SOURCE_WIDTH)
    ]
    # 元の縦粒度を保ち、横方向だけ線形補間で2倍に伸ばす。
    return bytes(
        round(base + value)
        for y in range(HEIGHT) for x in range(SOURCE_WIDTH)
        for value in (
            grain[y * SOURCE_WIDTH + x],
            (grain[y * SOURCE_WIDTH + x]
             + grain[y * SOURCE_WIDTH + (x + 1) % SOURCE_WIDTH]) / 2.0,
        )
        for base in (213, 218, 223)
    )


def png(data):
    def chunk(kind, payload):
        return (struct.pack(">I", len(payload)) + kind + payload
                + struct.pack(">I", zlib.crc32(kind + payload)))

    rows = b"".join(b"\0" + data[y * WIDTH * 3:(y + 1) * WIDTH * 3] for y in range(HEIGHT))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", WIDTH, HEIGHT, 8, 2, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    data = pixels()
    OUTPUT.with_suffix(".rgb").write_bytes(data)
    OUTPUT.with_suffix(".png").write_bytes(png(data))

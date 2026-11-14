# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""銀の旋盤加工面と黒い外周を持つノブの静止画像を生成する。"""

import math
from pathlib import Path
import struct
import zlib

SIZE = 72
SAMPLES = 4
RADIUS = 22 * 0.92 * 1.2
FACE_RADIUS = 17 * 0.92 * 1.2
BASE_HEIGHT = 2.0
BEVEL = 1.6
LIGHT_AZIMUTH = -math.pi / 2
LIGHT_TILT = math.radians(70)
OUTPUT = Path(__file__).resolve().parents[2] / "src/editor/knob-body"


def light_samples():
    # 上方の面光源を傾き空間で積分する。全光線の投影先は下側に限定される。
    # 発光面の縁を弱め、半影の外縁にも明瞭な境界を作らない。
    for i in range(96):
        r2 = (i + 0.5) / 96
        angle = i * math.pi * (3 - math.sqrt(5))
        spread = 2.0 * math.sqrt(r2)
        sx = -math.tan(LIGHT_TILT) * math.cos(LIGHT_AZIMUTH) + spread * math.cos(angle)
        sy = -math.tan(LIGHT_TILT) * math.sin(LIGHT_AZIMUTH) + spread * math.sin(angle)
        length = math.sqrt(sx * sx + sy * sy + 1)
        light = (-sx / length, -sy / length, 1 / length)
        half_length = math.sqrt(2 + 2 * light[2])
        half = (light[0] / half_length, light[1] / half_length,
                (light[2] + 1) / half_length)
        yield sx, sy, light, half, 1 - r2


LIGHTS = tuple(light_samples())
LIGHT_WEIGHT = sum(sample[4] for sample in LIGHTS)


def environment_reflections():
    # 主光源と周囲の反射面。各面の輝度を個別に積分し、暗い環境との対比を残す。
    yield from ((half, weight * 750.0 / LIGHT_WEIGHT)
                for _, _, _, half, weight in LIGHTS)
    # 入射方向x/z, y/z・面の横幅/縦幅・反射輝度。幅の異なる面が複数の帯を作る。
    for x, y, width, height, brightness in (
        (-1.4, -0.5, 0.5, 1.6, 280.0),
        (1.6, 0.3, 0.2, 1.8, 170.0),
        (0.6, 1.8, 1.4, 0.6, 100.0),
    ):
        for row in range(4):
            for column in range(4):
                lx = x + width * ((column + 0.5) / 4 - 0.5)
                ly = y + height * ((row + 0.5) / 4 - 0.5)
                length = math.sqrt(lx * lx + ly * ly + 1)
                hz = 1 / length + 1
                half_length = math.sqrt(2 * hz)
                yield (lx / length / half_length, ly / length / half_length,
                       hz / half_length), brightness / 16


REFLECTIONS = tuple(environment_reflections())


def projected_solid(x, y, sx, sy, bottom, height, radius, taper=0.0):
    """円柱／円錐台の投影内かを、線形に変化する円の包絡で判定する。"""
    x, y = x - sx * bottom, y - sy * bottom
    dx, dy = sx * height, sy * height
    curvature = dx * dx + dy * dy - taper * taper
    if curvature <= 0.0:
        # 広い面光源の正面寄りの光線では、円錐台の端面が投影の外縁になる。
        return (x * x + y * y < radius * radius
                or (x - dx) ** 2 + (y - dy) ** 2 < (radius - taper) ** 2)
    t = max(0.0, min(1.0, (x * dx + y * dy - radius * taper)
                    / curvature))
    return (x - t * dx) ** 2 + (y - t * dy) ** 2 < (radius - t * taper) ** 2


def shadow(x, y, panel):
    # パネルには台座と銀面、台座の天面には銀面だけが影を落とす。
    covered = sum(weight for sx, sy, _, _, weight in LIGHTS
                  if (panel and projected_solid(x, y, sx, sy, 0, BASE_HEIGHT, RADIUS))
                  or projected_solid(x, y, sx, sy, BASE_HEIGHT if panel else 0,
                                     BEVEL, FACE_RADIUS, BEVEL))
    return covered / LIGHT_WEIGHT


def surface(x, y):
    radius = math.hypot(x, y)
    if radius > RADIUS:
        # 接地部の狭い環境光遮蔽を落ち影へ重ね、台座との間に明るい縁を作らない。
        contact = 0.38 * math.exp(-(radius - RADIUS) / 0.7)
        return (8.0, 12.0, 16.0, contact + 0.42 * shadow(x, y, panel=True))

    angle = math.atan2(y, x)
    light = math.cos(angle - LIGHT_AZIMUTH)
    if radius > FACE_RADIUS:
        # 丸い肩の反射と銀面の投影影。全周の接触暗縁は0.25pxに限定する。
        shoulder = math.exp(-((radius - (RADIUS - 1.1)) / 0.85) ** 2)
        rim = math.exp(-((radius - (RADIUS - 0.22)) / 0.18) ** 2)
        contact = math.exp(-((radius - FACE_RADIUS) / 0.25) ** 2)
        level = 27.0 + 9.0 * light + shoulder * (24.0 + 28.0 * light)
        level += rim * (13.0 + 15.0 * light)
        level = 12.0 + (level - 12.0) * (1.0 - 0.65 * shadow(x, y, panel=False))
        level -= contact * 8.0
        level = max(6.0, level)
        return (level, level + 2.0, level + 3.0, 1.0)

    machining = 0.65 * math.sin(radius * 19.0) + 0.35 * math.sin(radius * 37.0)
    if radius > FACE_RADIUS - BEVEL:
        # 幅と高さが同じ45°の面取り。影と同じ面光源、正面視点で反射を積分。
        normal = (math.cos(angle) / math.sqrt(2), math.sin(angle) / math.sqrt(2),
                  1 / math.sqrt(2))
        diffuse = sum(max(0.0, sum(n * l for n, l in zip(normal, direction))) * w
                      for _, _, direction, _, w in LIGHTS) / LIGHT_WEIGHT
        specular = sum(max(0.0, sum(n * h for n, h in zip(normal, half))) ** 48 * brightness
                       for half, brightness in REFLECTIONS)
        level = 58.0 + 103.0 * diffuse + 1.05 * specular + machining
    else:
        # 天面はごく浅い凸面。旋盤目の半径方向／接線方向で粗さを変え、
        # 視線と入射光の半角ベクトルから反射を求める。明暗模様は直接描かない。
        slope = 0.12 * radius / (FACE_RADIUS - BEVEL)
        nz = 1 / math.sqrt(1 + slope * slope)
        nr = slope * nz
        cx, cy = math.cos(angle), math.sin(angle)
        normal = (nr * cx, nr * cy, nz)
        tangent_roughness = 0.12 + 0.43 * math.exp(-(radius / 0.9) ** 2)
        specular = 0.0
        for half, brightness in REFLECTIONS:
            nh = sum(n * h for n, h in zip(normal, half))
            hr = (half[0] * cx + half[1] * cy) * nz - half[2] * nr
            ht = -half[0] * cy + half[1] * cx
            specular += math.exp(-((hr / 0.55) ** 2 + (ht / tangent_roughness) ** 2)
                                 / (nh * nh)) * brightness
        # 中央の等方化で反射ローブが広がる分の増光を、粗さの比で抑える。
        specular *= 0.12 / tangent_roughness
        # 明るい室内の映り込みを基底とし、銀面の暗部と反射帯の露出を揃える。
        level = 145.0 + 0.85 * specular + machining
    if radius > FACE_RADIUS - 0.18:
        level -= 22.0
    return tuple(min(255.0, max(0.0, level + c)) for c in (0.0, 1.0, 2.0)) + (1.0,)


def pixels():
    output = bytearray()
    for y in range(SIZE):
        for x in range(SIZE):
            accumulated = [0.0, 0.0, 0.0, 0.0]
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    rgba = surface(x + (sx + 0.5) / SAMPLES - SIZE / 2,
                                   y + (sy + 0.5) / SAMPLES - SIZE / 2)
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

    rows = b"".join(b"\0" + data[y * SIZE * 4:(y + 1) * SIZE * 4]
                    for y in range(SIZE))
    return (b"\x89PNG\r\n\x1a\n"
            + chunk(b"IHDR", struct.pack(">IIBBBBB", SIZE, SIZE, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(rows, 9)) + chunk(b"IEND", b""))


if __name__ == "__main__":
    data = pixels()
    OUTPUT.with_suffix(".rgba").write_bytes(data)
    OUTPUT.with_suffix(".png").write_bytes(png(data))

# Copyright (C) 2026 Yusuke Oda
# SPDX-License-Identifier: AGPL-3.0-only
"""帯域別の周期波と4点Lagrange補間係数を生成する。通常のビルド時は実行しない。"""
from pathlib import Path

import numpy as np

ROOT = Path(__file__).resolve().parents[1] / "src/oscillator"
BANDS = np.unique(np.floor(2 ** (np.arange(29) / 2)).astype(int))


def generate():
    data = bytearray()
    # 2^-16〜2^-2の各オクターブ先頭で選ぶ帯域。端の項目を範囲外にも使う。
    ids = [max(0, int(np.searchsorted(BANDS * 2.0**e, 0.5, side="left")) - 1)
           for e in range(-16, -1)]
    records = ["pub(super) const BAND_IDS: [u8; 15] = [" + ", ".join(map(str, ids)) + "];"]
    for wave in ["saw", "square", "triangle"]:
        rows = []
        for harmonic in BANDS:
            length = (int(max(4096, 8*harmonic)) + 3) // 4 * 4
            k = np.arange(1, harmonic + 1)
            if wave == "saw":
                amplitude = -2 / (np.pi*k)
            elif wave == "square":
                amplitude = np.where(k % 2, 4 / (np.pi*k), 0.)
            else:
                amplitude = np.where(k % 2, 8*np.where(k % 4 == 1, 1., -1.) / (np.pi*k)**2, 0.)
            spectrum = np.zeros(length // 2 + 1, complex)
            spectrum[1:harmonic + 1] = -1j * amplitude * length / 2
            values = np.fft.irfft(spectrum, length).astype(np.float32).astype(np.float64)
            values[0] = values[length // 2] = 0
            end = length // 2 if wave == "saw" else length // 4
            # 対称性による端点折返しにも同じ式を使えるよう、端点の区間も保存する。
            padded = values[np.arange(-1, end + 3) % length]
            a, b, c, d = padded[:-3], padded[1:-2], padded[2:-1], padded[3:]
            coefficients = np.column_stack((
                b, (-2*a - 3*b + 6*c - d)/6, (a - 2*b + c)/2, (-a + 3*b - 3*c + d)/6,
            )).astype("<f4").astype("<f8")
            offset = len(data)
            data.extend(coefficients.tobytes())
            rows.append(f"    Table {{ offset: {offset}, length: {length}.0, "
                        f"harmonic: {harmonic}.0 }},")
        records.append(f"#[rustfmt::skip]\npub(super) static {wave.upper()}: [Table; {len(BANDS)}] = [\n"
                       + "\n".join(rows) + "\n];")
    ROOT.joinpath("wavetable.bin").write_bytes(data)
    ROOT.joinpath("wavetable_data.rs").write_text(
        "// Copyright (C) 2026 Yusuke Oda\n// SPDX-License-Identifier: AGPL-3.0-only\n"
        "// scripts/generate-wavetables.pyによる生成値。手動編集しない。\n"
        "use super::Table;\n" + "\n".join(records) + "\n")
    print(len(data), "bytes")


if __name__ == "__main__":
    generate()

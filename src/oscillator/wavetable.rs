// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 帯域ごとのFourier級数を、対称性と事前計算した4点Lagrange補間で読む。

use super::Waveform;

#[path = "wavetable_data.rs"]
mod data;

static COEFFICIENTS: &[u8] = include_bytes!("wavetable.bin");

struct Table {
    /// 各区間4個のlittle-endian f32係数を格納した位置（バイト）。
    offset: usize,
    /// 対称性で省略する前の、1周期の区間数。
    length: f64,
    /// このテーブルに含める最高倍音の番号。
    harmonic: f64,
    /// 補間区間の内部極値を含むピーク上限。
    peak: f64,
}

impl Table {
    #[inline]
    fn read_sample(&self, phase: f64) -> f64 {
        let position = phase * self.length;
        let index = position as usize;
        let t = position - index as f64;
        let at = self.offset + index * 16;
        let bytes: &[u8; 16] = COEFFICIENTS[at..at + 16].try_into().unwrap();
        let coefficient = |i| f64::from(f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()));
        coefficient(0) + t * (coefficient(4) + t * (coefficient(8) + t * coefficient(12)))
    }
}

fn tables(wave: Waveform) -> &'static [Table] {
    match wave {
        Waveform::Sawtooth => &data::SAW,
        Waveform::Square => &data::SQUARE,
        Waveform::Triangle => &data::TRIANGLE,
        _ => unreachable!("帯域別テーブルはSaw/Square/Triangle専用"),
    }
}

/// 周波数・波形変更時に選ぶ読出し設定。OscillatorStateがユニゾンごとに保持する。
#[derive(Clone, Copy)]
pub(super) struct Selection {
    /// 高域側の倍音を多く含むテーブルの番号。
    index: usize,
    /// 直前の帯域（倍音の少ないテーブル）への混合率。
    mix: f64,
    /// raw出力をピーク1へ収める倍率。Nyquist直前では基音もフェードする。
    normalization: f64,
}

impl Selection {
    pub const EMPTY: Self = Self {
        index: 0,
        mix: 0.0,
        normalization: 0.0,
    };

    /// 正規化周波数Hz/fsから選択し、音量処理で戻すピーク倍率も返す。
    /// incrementは剰余を取る前の非負の周波数比。1以上は1で代用できる。
    #[inline(never)] // 帯域探索は入力変更時だけ行い、毎サンプルの入力比較へ展開しない。
    pub fn new(wave: Waveform, increment: f64) -> (Self, f64) {
        let tables = tables(wave);
        let index = tables
            .partition_point(|t| t.harmonic * increment < 0.5)
            .saturating_sub(1);
        let rich = &tables[index];
        let poor = &tables[index.saturating_sub(1)];
        // 最高倍音がNyquistを超える前に、最後の10%の区間で除去し終える。
        let mix = ((increment * rich.harmonic * 2.0 - 0.9) * 10.0).clamp(0.0, 1.0);
        let gain = if index == 0 { 1.0 - mix } else { 1.0 };
        let mix = if index == 0 { 0.0 } else { mix };
        let peak = rich.peak + (poor.peak - rich.peak) * mix;
        (
            Self {
                index,
                mix,
                normalization: gain / peak,
            },
            peak,
        )
    }

    #[inline(never)] // 帯域別の参照を分離し、既存のSine/Noiseのループを小さく保つ。
    pub fn generate_sample(&self, wave: Waveform, phase: f64) -> f64 {
        let half = 0.5 - (phase - 0.5).abs();
        let folded = if wave == Waveform::Sawtooth {
            half
        } else {
            0.25 - (half - 0.25).abs()
        };
        let tables = tables(wave);
        let mut value = tables[self.index].read_sample(folded);
        if self.mix != 0.0 {
            value += (tables[self.index - 1].read_sample(folded) - value) * self.mix;
        }
        value * (self.normalization.copysign(0.5 - phase))
    }
}

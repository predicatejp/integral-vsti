// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! テーブル参照とは独立した、回帰テスト用のFourier級数。
use super::{Waveform, wavetable};

fn coefficients(wave: Waveform, increment: f64) -> Vec<f64> {
    let mut bands: Vec<_> = (0..=28)
        .map(|i| 2.0_f64.powf(f64::from(i) / 2.0) as usize)
        .collect();
    bands.dedup();
    let index = bands
        .iter()
        .rposition(|&h| h as f64 * increment < 0.5)
        .unwrap_or(0);
    let hi = bands[index];
    let lo = bands[index.saturating_sub(1)];
    let fade = ((increment * hi as f64 * 2.0 - 0.9) * 10.0).clamp(0.0, 1.0);
    (1..=hi)
        .map(|h| {
            let x = std::f64::consts::PI * h as f64;
            let a = match wave {
                Waveform::Sawtooth => -2.0 / x,
                Waveform::Square if h % 2 == 1 => 4.0 / x,
                Waveform::Triangle if h % 2 == 1 => {
                    8.0 * if h % 4 == 1 { 1.0 } else { -1.0 } / (x * x)
                }
                _ => 0.0,
            };
            a * if index == 0 || h > lo {
                1.0 - fade
            } else {
                1.0
            }
        })
        .collect()
}

/// 残す倍音の振幅を維持した、生波形の期待値。
pub fn audio_sample(wave: Waveform, phase: f64, increment: f64) -> f64 {
    if wave == Waveform::Sine {
        return (std::f64::consts::TAU * phase).sin();
    }
    coefficients(wave, increment)
        .iter()
        .enumerate()
        .map(|(i, a)| a * (std::f64::consts::TAU * (i + 1) as f64 * phase).sin())
        .sum()
}

/// 接続試験用の単発評価。保持位相の進行と段間接続を、実際の読出し波形で照合する。
pub fn raw_sample(wave: Waveform, phase: f64, increment: f64) -> f64 {
    if wave == Waveform::Sine {
        super::sine_sample(phase)
    } else {
        wavetable::Selection::new(wave, increment).generate_sample(wave, phase)
    }
}

pub fn rms(wave: Waveform, increment: f64) -> f64 {
    if wave == Waveform::Sine {
        return std::f64::consts::FRAC_1_SQRT_2;
    }
    (coefficients(wave, increment)
        .iter()
        .map(|a| a * a)
        .sum::<f64>()
        / 2.0)
        .sqrt()
}

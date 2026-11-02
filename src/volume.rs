// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器の生波形に対する実効値補正・ユニゾン合成・Vol・定位。

use crate::oscillator::{PanningGains, Waveform};

/// 波形の実効値補正とユニゾン補正にVolの線形倍率を掛ける。設定変更時に計算する。
/// countは検証済みの実発音数（1〜7）。
pub fn gain(waveform: Waveform, count: usize, volume: f64) -> f64 {
    let peak_per_rms = match waveform {
        Waveform::Sine => std::f64::consts::SQRT_2,
        Waveform::Triangle | Waveform::Sawtooth | Waveform::Noise => 1.732_050_807_568_877_2,
        Waveform::Square => 1.0,
    };
    peak_per_rms * volume / (count as f64).sqrt()
}

/// ピーク1の生波形を、残す倍音の元の振幅へ戻す。位相変調用rawの使用後に呼ぶ。
#[inline]
pub fn restore_peak_sample(values: &mut [f64], peaks: &[f64; 7]) {
    for (value, peak) in values.iter_mut().zip(peaks) {
        *value *= peak;
    }
}

/// 同時刻のユニゾン波形を定位・加算し、計算済みの音量倍率を適用する。
/// valuesは最大7要素。空の場合は無音。
#[inline]
pub fn process_sample(values: &[f64], panning: &PanningGains, gain: f64) -> [f64; 2] {
    let mut sum = [0.0; 2];
    for (&value, gains) in values.iter().zip(panning) {
        sum[0] += value * gains[0];
        sum[1] += value * gains[1];
    }
    sum.map(|value| value * gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn volume_is_independent_of_raw_samples_and_scales_after_unison() {
        for wave in [
            Waveform::Sine,
            Waveform::Triangle,
            Waveform::Square,
            Waveform::Sawtooth,
            Waveform::Noise,
        ] {
            for count in 1..=7 {
                let raw = [0.1, -0.3, 0.5, -0.7, 0.9, -1.0, 1.0];
                let gains = crate::oscillator::panning_gains(count, 1.0, 0.2);
                let reference = process_sample(&raw[..count], &gains, gain(wave, count, 1.0));
                for volume in [
                    0.0,
                    10.0_f64.powf(-48.0 / 20.0),
                    0.5,
                    1.0,
                    10.0_f64.powf(12.0 / 20.0),
                ] {
                    let actual = process_sample(&raw[..count], &gains, gain(wave, count, volume));
                    for ch in 0..2 {
                        assert!((actual[ch] - reference[ch] * volume).abs() < 1e-15);
                    }
                }
            }
        }
    }
}

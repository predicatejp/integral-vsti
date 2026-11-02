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
}

impl Table {
    #[inline]
    fn read_sample(&self, phase: f64) -> f64 {
        let position = phase * self.length;
        // SAFETY: 呼出し側は検証済みの位相[0,1]を[0,0.5]へ折り返す。
        // 生成済みテーブルのlengthは最大131072なのでpositionは有限の[0,65536]。
        let index = unsafe { position.to_int_unchecked::<i32>() };
        let t = position - f64::from(index);
        let at = self.offset + index as usize * 16;
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
    /// Nyquist直前で基音を0へ下げる倍率。他の帯域は1。
    gain: f64,
}

impl Selection {
    pub const EMPTY: Self = Self {
        index: 0,
        mix: 0.0,
        gain: 0.0,
    };

    /// 正規化周波数Hz/fsから帯域とクロスフェード係数を選ぶ。
    /// incrementは剰余を取る前の非負の周波数比。1以上は1で代用できる。
    #[inline(never)] // 帯域選択は入力変更時だけ行い、毎サンプルの入力比較へ展開しない。
    pub fn new(wave: Waveform, increment: f64) -> Self {
        let tables = tables(wave);
        // 正規化周波数の指数で候補IDを引く。1007は2^-16のbiased exponent。
        // 各オクターブ内の追加の境界は最大1個なので、実周波数で1回だけ補正する。
        let exponent = ((increment.to_bits() >> 52) & 0x7ff) as usize;
        let bucket = exponent.saturating_sub(1007).min(data::BAND_IDS.len() - 1);
        let candidate = usize::from(data::BAND_IDS[bucket]);
        let index =
            candidate.saturating_sub(usize::from(tables[candidate].harmonic * increment >= 0.5));
        let rich = &tables[index];
        // 最高倍音がNyquistを超える前に、最後の10%の区間で除去し終える。
        let mix = ((increment * rich.harmonic * 2.0 - 0.9) * 10.0).clamp(0.0, 1.0);
        let gain = if index == 0 { 1.0 - mix } else { 1.0 };
        let mix = if index == 0 { 0.0 } else { mix };
        Self { index, mix, gain }
    }

    /// phaseは発振器の保持位相または変調後の、有限の[0,1]。
    #[inline(always)] // 帯域別のユニゾンループ内へ展開し、サンプルごとの呼出しを省く。
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
        value * (self.gain.copysign(0.5 - phase))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[cfg_attr(miri, ignore)] // 全指数の密な走査は通常のCPUテストで行う。
    fn band_id_lookup_matches_binary_search() {
        for wave in [Waveform::Sawtooth, Waveform::Square, Waveform::Triangle] {
            let check = |increment| {
                let reference = tables(wave)
                    .partition_point(|t| t.harmonic * increment < 0.5)
                    .saturating_sub(1);
                assert_eq!(
                    Selection::new(wave, increment).index,
                    reference,
                    "increment={increment:?}"
                );
            };
            for increment in [-0.0, 0.0, f64::from_bits(1), f64::MIN_POSITIVE, f64::MAX] {
                check(increment);
            }
            for table in tables(wave) {
                let boundary = 0.5 / table.harmonic;
                for value in [boundary.next_down(), boundary, boundary.next_up()] {
                    check(value);
                }
            }
            for exponent in 0..2047_u64 {
                for mantissa in [0, 1, 1 << 50, 1 << 51, (1 << 52) - 2, (1 << 52) - 1] {
                    check(f64::from_bits((exponent << 52) | mantissa));
                }
            }
        }
    }

    #[test]
    fn generated_table_positions_fit_fast_integer_conversion() {
        for wave in [Waveform::Sawtooth, Waveform::Square, Waveform::Triangle] {
            let end = if wave == Waveform::Sawtooth {
                0.5
            } else {
                0.25
            };
            for table in tables(wave) {
                assert!((4.0..=131072.0).contains(&table.length));
                assert_eq!(table.length.fract(), 0.0);
                assert!(table.length * end <= 65536.0);
                for phase in [0.0, end / 2.0, end] {
                    assert!(table.read_sample(phase).is_finite());
                }
            }
        }
    }
}

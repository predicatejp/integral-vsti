// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 帯域ごとのFourier級数を、対称性と事前計算した4点Lagrange補間で読む。

use super::Waveform;

#[path = "wavetable_data.rs"]
mod data;

static COEFFICIENTS: &[u8] = include_bytes!("wavetable.bin");

struct Table {
    /// 各区間4個のlittle-endian f64係数を格納した位置（バイト）。
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
        let at = self.offset + index as u32 as usize * 32;
        let bytes: &[u8; 32] = COEFFICIENTS[at..at + 32].try_into().unwrap();
        let coefficient = |i| f64::from_le_bytes(bytes[i..i + 8].try_into().unwrap());
        coefficient(0) + t * (coefficient(8) + t * (coefficient(16) + t * coefficient(24)))
    }
}

/// 同一時刻の2ユニゾンを同じHorner順で評価する。
#[inline]
fn read_pair_sample(
    a: &Table,
    b: &Table,
    phase: std::arch::x86_64::__m128d,
) -> std::arch::x86_64::__m128d {
    use std::arch::x86_64::*;
    // SAFETY: 製品必須のAVX2を使う。位相の折返しと生成済みlengthから
    // positionは有限の[0,65536]。各読出しは範囲検査済みの32byteセル内。
    unsafe {
        let position = _mm_mul_pd(phase, _mm_set_pd(b.length, a.length));
        let indexes = _mm_cvttpd_epi32(position);
        let i = _mm_cvtsi128_si32(indexes) as u32 as usize;
        let j = _mm_cvtsi128_si32(_mm_shuffle_epi32::<0x55>(indexes)) as u32 as usize;
        let at_a = a.offset + i * 32;
        let at_b = b.offset + j * 32;
        let bytes_a = &COEFFICIENTS[at_a..at_a + 32];
        let bytes_b = &COEFFICIENTS[at_b..at_b + 32];
        let a = _mm256_loadu_pd(bytes_a.as_ptr().cast::<f64>());
        let b = _mm256_loadu_pd(bytes_b.as_ptr().cast::<f64>());
        let lo = _mm256_unpacklo_pd(a, b);
        let hi = _mm256_unpackhi_pd(a, b);
        let c0 = _mm256_castpd256_pd128(lo);
        let c1 = _mm256_castpd256_pd128(hi);
        let c2 = _mm256_extractf128_pd::<1>(lo);
        let c3 = _mm256_extractf128_pd::<1>(hi);
        let t = _mm_sub_pd(position, _mm_cvtepi32_pd(indexes));
        _mm_add_pd(
            c0,
            _mm_mul_pd(
                t,
                _mm_add_pd(c1, _mm_mul_pd(t, _mm_add_pd(c2, _mm_mul_pd(t, c3)))),
            ),
        )
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
    /// 周波数変更時に選ぶ倍音の多い帯域と、その隣の帯域。
    hi: &'static Table,
    lo: &'static Table,
    /// 直前の帯域（倍音の少ないテーブル）への混合率。
    mix: f64,
    /// Nyquist直前で基音を0へ下げる倍率。他の帯域は1。
    gain: f64,
}

impl Selection {
    pub const EMPTY: Self = Self {
        hi: &data::SAW[0],
        lo: &data::SAW[0],
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
        Self {
            hi: rich,
            lo: &tables[index.saturating_sub(1)],
            mix,
            gain,
        }
    }

    /// 同一時刻の2ユニゾンを並列に読む。各位相は有限の[0,1]。
    #[inline(always)]
    pub fn generate_pair_sample(a: &Self, b: &Self, wave: Waveform, phases: [f64; 2]) -> [f64; 2] {
        use std::arch::x86_64::*;
        // SAFETY: x86_64必須のSSE2。load/storeは2要素の配列内。
        unsafe {
            let phase = _mm_loadu_pd(phases.as_ptr());
            let half_cycle = _mm_set1_pd(0.5);
            let sign_bit = _mm_set1_pd(-0.0);
            let half = _mm_sub_pd(
                half_cycle,
                _mm_andnot_pd(sign_bit, _mm_sub_pd(phase, half_cycle)),
            );
            let folded = if wave == Waveform::Sawtooth {
                half
            } else {
                let quarter = _mm_set1_pd(0.25);
                _mm_sub_pd(quarter, _mm_andnot_pd(sign_bit, _mm_sub_pd(half, quarter)))
            };
            let mut value = read_pair_sample(a.hi, b.hi, folded);
            if a.mix != 0.0 || b.mix != 0.0 {
                let mix = _mm_set_pd(b.mix, a.mix);
                let low = read_pair_sample(a.lo, b.lo, folded);
                let blended = _mm_add_pd(value, _mm_mul_pd(_mm_sub_pd(low, value), mix));
                let changed = _mm_cmpneq_pd(mix, _mm_setzero_pd());
                value = _mm_or_pd(_mm_and_pd(changed, blended), _mm_andnot_pd(changed, value));
            }
            let signs = _mm_and_pd(sign_bit, _mm_sub_pd(half_cycle, phase));
            let gain = _mm_or_pd(_mm_set_pd(b.gain, a.gain), signs);
            let mut result = [0.0; 2];
            _mm_storeu_pd(result.as_mut_ptr(), _mm_mul_pd(value, gain));
            result
        }
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
        let mut value = self.hi.read_sample(folded);
        if self.mix != 0.0 {
            value += (self.lo.read_sample(folded) - value) * self.mix;
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
                assert!(
                    std::ptr::eq(Selection::new(wave, increment).hi, &tables(wave)[reference]),
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
    fn paired_unison_matches_scalar_cells_and_crossfades() {
        for wave in [Waveform::Sawtooth, Waveform::Square, Waveform::Triangle] {
            for table in tables(wave) {
                for factor in [0.88, 0.91, 0.95, 0.999, 1.01] {
                    let a = Selection::new(wave, factor * 0.5 / table.harmonic);
                    let b = Selection::new(wave, factor * 0.5 / (table.harmonic * 1.013));
                    let steps = if cfg!(miri) { 4 } else { 1024 };
                    for n in 0..=steps {
                        let phases = [
                            f64::from(n) / f64::from(steps),
                            f64::from((n * 137) % (steps + 1)) / f64::from(steps),
                        ];
                        let actual = Selection::generate_pair_sample(&a, &b, wave, phases);
                        let expected = [
                            a.generate_sample(wave, phases[0]),
                            b.generate_sample(wave, phases[1]),
                        ];
                        assert_eq!(actual.map(f64::to_bits), expected.map(f64::to_bits));
                    }
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

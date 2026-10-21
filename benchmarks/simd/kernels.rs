// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! 実験専用。run-simd.pyがSSE2/AVX2の名前と幅を置換するテンプレート。
//! 全バイナリを同じISAの自動ベクトル化有効な対照群と比較する。FMAは使わない。
use crate::oscillator::Waveform;
use std::arch::x86_64::*;
pub const LANES: usize = WIDTH;

// SAFETY: 呼出側の配列はLANES要素以上。runnerが必要なISAを事前検出する。
// unaligned load/storeだけを使用し、範囲外アクセスや他スレッドとの共有はない。
#[inline]
unsafe fn select(mask: VECTOR, yes: VECTOR, no: VECTOR) -> VECTOR {
    unsafe { V_or_pd(V_and_pd(mask, yes), V_andnot_pd(mask, no)) }
}

#[inline]
unsafe fn trunc(v: VECTOR) -> VECTOR {
    // 入力は位相・象限・テーブル添字の非負かつi32範囲内の値だけ。
    unsafe { V_cvtepi32_pd(V_cvttpd_epi32(v)) }
}

#[inline]
unsafe fn wave(v: VECTOR, kind: Waveform) -> VECTOR {
    unsafe {
        let one = V_set1_pd(1.0);
        match kind {
            Waveform::Sawtooth => V_sub_pd(V_mul_pd(V_set1_pd(2.0), v), one),
            Waveform::Square => select(LT(v, V_set1_pd(0.5)), one, V_set1_pd(-1.0)),
            Waveform::Triangle => {
                let shifted = V_add_pd(v, V_set1_pd(0.25));
                let x = V_sub_pd(V_sub_pd(shifted, trunc(shifted)), V_set1_pd(0.5));
                V_sub_pd(
                    one,
                    V_mul_pd(V_set1_pd(4.0), V_andnot_pd(V_set1_pd(-0.0), x)),
                )
            }
            Waveform::Sine => {
                let position = V_mul_pd(v, V_set1_pd(4.0));
                let quadrant = trunc(position);
                let fraction = V_sub_pd(position, quadrant);
                let parity = V_sub_pd(
                    quadrant,
                    V_mul_pd(V_set1_pd(2.0), trunc(V_mul_pd(quadrant, V_set1_pd(0.5)))),
                );
                let folded = select(LT(parity, one), fraction, V_sub_pd(one, fraction));
                let table_position = V_mul_pd(folded, V_set1_pd(1024.0));
                let index = V_min_pd(trunc(table_position), V_set1_pd(1023.0));
                let (lo, hi) = table_pair(index);
                let value = V_add_pd(
                    lo,
                    V_mul_pd(V_sub_pd(hi, lo), V_sub_pd(table_position, index)),
                );
                V_xor_pd(
                    value,
                    V_andnot_pd(LT(position, V_set1_pd(2.0)), V_set1_pd(-0.0)),
                )
            }
        }
    }
}

#[inline]
unsafe fn table_pair(index: VECTOR) -> (VECTOR, VECTOR) {
    unsafe { TABLE_PAIR }
}

#[inline]
/// SAFETY: 全位相は[0, 1)。gatherの添字範囲をこの契約で保証する。
pub unsafe fn generate_unison_sample(phases: &[f64], kind: Waveform) -> f64 {
    assert!(phases.len() <= 7);
    let mut values = [0.0; 7];
    let mut i = 0;
    while i + LANES <= phases.len() {
        // SAFETY: 完全なSIMD幅が残る範囲だけ読み書きする。
        unsafe {
            V_storeu_pd(
                values.as_mut_ptr().add(i),
                wave(V_loadu_pd(phases.as_ptr().add(i)), kind),
            );
        }
        i += LANES;
    }
    for j in i..phases.len() {
        values[j] = kind.unit_sample(phases[j]);
    }
    // 水平加算による丸め順変更を混ぜず、既存のユニゾン順で加算する。
    values[..phases.len()].iter().sum()
}

#[inline]
pub fn update_phase_sample(phases: &mut [f64], increments: &[f64]) {
    assert!(increments.len() >= phases.len());
    let mut i = 0;
    while i + LANES <= phases.len() {
        // SAFETY: 両配列の完全なSIMD幅だけ使用。和は0以上2以下。
        unsafe {
            let p = V_add_pd(
                V_loadu_pd(phases.as_ptr().add(i)),
                V_loadu_pd(increments.as_ptr().add(i)),
            );
            V_storeu_pd(phases.as_mut_ptr().add(i), V_sub_pd(p, trunc(p)));
        }
        i += LANES;
    }
    for j in i..phases.len() {
        phases[j] = (phases[j] + increments[j]).fract();
    }
}

/// 同じ時刻の独立したSVFを連続配置した作業データ。積分器も各レーンで独立。
#[derive(Clone, Copy)]
pub struct SvfLanes {
    pub x: [f64; LANES],
    pub state: SvfLaneState,
    pub g: [f64; LANES],
    pub damping_plus_g: [f64; LANES],
    pub denominator: [f64; LANES],
}

/// レーンごとの積分器履歴。処理はSvfLanesが担当する。
#[derive(Clone, Copy)]
pub struct SvfLaneState {
    pub s1: [f64; LANES],
    pub s2: [f64; LANES],
}

impl SvfLanes {
    #[inline]
    pub fn process_sample(&mut self, output: usize) -> ([f64; LANES], [bool; LANES]) {
        let mut result = [0.0; LANES];
        let mut lows = [0.0; LANES];
        let mut bands = [0.0; LANES];
        let mut highs = [0.0; LANES];
        if cfg!(scalar_batch) {
            for (i, result) in result.iter_mut().enumerate() {
                let high =
                    (self.x[i] - self.damping_plus_g[i] * self.state.s1[i] - self.state.s2[i])
                        / self.denominator[i];
                let band = self.state.s1[i] + self.g[i] * high;
                let low = self.state.s2[i] + self.g[i] * band;
                self.state.s1[i] = 2.0 * band - self.state.s1[i];
                self.state.s2[i] = 2.0 * low - self.state.s2[i];
                *result = [low, band, high][output];
                lows[i] = low;
                bands[i] = band;
                highs[i] = high;
            }
        } else {
            // SAFETY: すべてLANES長の配列。係数は呼び出し側で準備済み。
            unsafe {
                let s1 = V_loadu_pd(self.state.s1.as_ptr());
                let s2 = V_loadu_pd(self.state.s2.as_ptr());
                let g = V_loadu_pd(self.g.as_ptr());
                let high = V_div_pd(
                    V_sub_pd(
                        V_sub_pd(
                            V_loadu_pd(self.x.as_ptr()),
                            V_mul_pd(V_loadu_pd(self.damping_plus_g.as_ptr()), s1),
                        ),
                        s2,
                    ),
                    V_loadu_pd(self.denominator.as_ptr()),
                );
                let band = V_add_pd(s1, V_mul_pd(g, high));
                let low = V_add_pd(s2, V_mul_pd(g, band));
                V_storeu_pd(
                    self.state.s1.as_mut_ptr(),
                    V_sub_pd(V_mul_pd(V_set1_pd(2.0), band), s1),
                );
                V_storeu_pd(
                    self.state.s2.as_mut_ptr(),
                    V_sub_pd(V_mul_pd(V_set1_pd(2.0), low), s2),
                );
                V_storeu_pd(result.as_mut_ptr(), [low, band, high][output]);
                V_storeu_pd(lows.as_mut_ptr(), low);
                V_storeu_pd(bands.as_mut_ptr(), band);
                V_storeu_pd(highs.as_mut_ptr(), high);
            }
        }
        let finite = std::array::from_fn(|i| {
            [
                lows[i],
                bands[i],
                highs[i],
                self.state.s1[i],
                self.state.s2[i],
            ]
            .iter()
            .all(|v| v.is_finite())
        });
        (result, finite)
    }
}

pub fn convert_samples(input: &[f64], left: &mut [f32], right: &mut [f32]) {
    assert!(left.len() >= input.len() && right.len() >= input.len());
    let mut i = 0;
    while i + LANES <= input.len() {
        // SAFETY: 入出力は同じ長さ。レーン数分を読み、f32へ変換して別々の出力へ書く。
        unsafe {
            let values = V_cvtpd_ps(V_loadu_pd(input.as_ptr().add(i)));
            STORE_FLOATS
        }
        i += LANES;
    }
    for j in i..input.len() {
        left[j] = input[j] as f32;
        right[j] = left[j];
    }
}

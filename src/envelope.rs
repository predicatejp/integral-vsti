// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 単位に依存せず、0〜1の正規化値を操作する線形AHDSR。

/// エンベロープの時間と正規化値をまとめたパラメーター。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvelopeParameters {
    /// 開始値からピーク値へ移行する時間（0..=10000 ms）。
    pub attack_ms: f64,
    /// 最大値1を維持する時間（0..=10000 ms）。
    pub hold_ms: f64,
    /// 最大値1からsustainへ下がる時間（0..=10000 ms）。
    pub decay_ms: f64,
    /// 減衰後に維持する値（0..=1）。最大値に対する倍率。
    pub sustain: f64,
    /// 値が0へ下がる時間（0..=10000 ms）。
    pub release_ms: f64,
}

/// 音源で使用する初期設定。時間はms、sustainはピークに対する倍率。
pub const DEFAULT_ENVELOPE: EnvelopeParameters = EnvelopeParameters {
    attack_ms: 10.0,
    hold_ms: 0.0,
    decay_ms: 100.0,
    sustain: 0.7,
    release_ms: 100.0,
};

impl EnvelopeParameters {
    /// 範囲外、NaN、無限大を拒否する。
    pub fn is_valid(&self) -> bool {
        (0.0..=10000.0).contains(&self.attack_ms)
            & (0.0..=10000.0).contains(&self.hold_ms)
            & (0.0..=10000.0).contains(&self.decay_ms)
            & (0.0..=1.0).contains(&self.sustain)
            & (0.0..=10000.0).contains(&self.release_ms)
    }
}

/// ノートオンからelapsed_ms経過した時点の値（0..=1）を返す。
/// start_valueはノートオン時の値、peak_valueは新しいピーク値。
/// attackは開始値に関わらずattack_ms全体を使う。sustainはピークに対する倍率。
/// note_off_msもノートオンからの経過時間。Noneならsustainを維持する。
/// 指定されたノートオフ時刻より前は通常の立ち上がり・減衰を計算し、
/// その時刻以降はノートオフ時の値からrelease_msかけて0へ線形に下げる。
/// パラメーター不正、負の時刻、NaN、無限大では0を返す。
/// 内部状態を持たず、毎回渡されたパラメーターから計算する。
#[allow(dead_code)] // 時刻指定の単体入口。Envelope内部は検証済みの設定を再利用する。
pub fn value(
    parameters: &EnvelopeParameters,
    elapsed_ms: f64,
    note_off_ms: Option<f64>,
    start_value: f64,
    peak_value: f64,
) -> f64 {
    let off = note_off_ms.unwrap_or(0.0);
    if !((0.0..=1.0).contains(&start_value)
        & (0.0..=1.0).contains(&peak_value)
        & parameters.is_valid()
        & (0.0..=f64::MAX).contains(&elapsed_ms)
        & (0.0..=f64::MAX).contains(&off))
    {
        return 0.0;
    }
    value_validated(parameters, elapsed_ms, note_off_ms, start_value, peak_value)
}

/// 検証済みの設定を使い、各段階を0〜1の線形ランプの積で合成する。
fn value_validated(
    parameters: &EnvelopeParameters,
    elapsed_ms: f64,
    note_off_ms: Option<f64>,
    start: f64,
    peak: f64,
) -> f64 {
    let off = note_off_ms.unwrap_or(f64::INFINITY);
    let held = elapsed_ms.min(off);
    let attack = ramp(held, parameters.attack_ms);
    let decay = ramp(
        held - parameters.attack_ms - parameters.hold_ms,
        parameters.decay_ms,
    );
    let release = ramp(elapsed_ms - off, parameters.release_ms);
    (start * (1.0 - attack) + peak * attack)
        * ((1.0 - decay) + parameters.sustain * decay)
        * (1.0 - release)
}

/// 0ms段階は時刻0で即時完了する。0/0のNaNもmin(1)で1へ変換する。
#[allow(clippy::manual_clamp)] // clampはNaNを保持するため、0msの境界の意味が異なる。
fn ramp(time: f64, duration: f64) -> f64 {
    (time / duration.abs()).min(1.0).max(0.0)
}

/// エンベロープの独立した時間・再トリガー履歴。データだけを保持する。
#[derive(Clone, Copy)]
struct EnvelopeState {
    /// 次サンプル時点の正規化値。再ノートオンの開始値に使う。
    current_value: f64,
    /// 現在の線形区間で1サンプル進めたときの増分。
    increment: f64,
    /// 次の段境界までのサンプル数。設定変更時の0は次回生成での再計算を表す。
    remaining: u64,
    /// 検証・値計算に使用した設定、レート、ピーク。変更時だけ再検証する。
    input: Option<(EnvelopeParameters, f64, f64)>,
    /// 直近のノートオン時の値。attack中は固定する。
    start_value: f64,
    /// 直近のノートオンから次サンプルまでのサンプル数。
    elapsed_samples: u64,
    /// ノートオフ位置。ノートオンからのサンプル数で保持する。
    note_off_sample: Option<u64>,
    /// releaseが完了するまでtrue。値0のattack中も進行する。
    active: bool,
    /// 処理済みのノートオン世代。
    note_generation: u64,
    /// 処理済みの即時停止世代。
    reset_generation: u64,
}

/// 音量・カットオフで共用するAHDSR。時間管理も各インスタンスで独立する。
#[derive(Clone, Copy)]
pub struct Envelope {
    /// このエンベロープ専用の内部状態。
    state: EnvelopeState,
}

impl Envelope {
    pub fn new() -> Self {
        Self {
            state: EnvelopeState {
                current_value: 0.0,
                increment: 0.0,
                remaining: 0,
                input: None,
                start_value: 0.0,
                elapsed_samples: 0,
                note_off_sample: None,
                active: false,
                note_generation: 0,
                reset_generation: 0,
            },
        }
    }

    /// 設定・レート・ピークを変更時に反映する。不正値では生成を止め、時間履歴を保つ。
    /// 値の再計算は次回生成へ遅らせ、同位置のNote Onは直前の音量から開始する。
    pub fn set_input(
        &mut self,
        parameters: &EnvelopeParameters,
        sample_rate: f64,
        peak: f64,
    ) -> bool {
        let input = (*parameters, sample_rate, peak);
        if self.state.input == Some(input) {
            return true;
        }
        self.state.remaining = 0;
        if !(parameters.is_valid()
            & (0.0..=1.0).contains(&peak)
            & sample_rate.is_finite()
            & (sample_rate > 0.0))
        {
            self.state.input = None;
            return false;
        }
        self.state.input = Some(input);
        true
    }

    /// ノート境界を反映し、設定済み入力から現在サンプルの正規化値を生成する。
    #[inline(always)]
    pub fn generate_sample(
        &mut self,
        gate: bool,
        note_generation: u64,
        reset_generation: u64,
    ) -> f64 {
        if self.state.input.is_none() {
            return 0.0;
        }
        let reset = reset_generation != self.state.reset_generation;
        if reset {
            self.state.active = false;
            self.state.current_value = 0.0;
        }
        let triggered = note_generation != self.state.note_generation && (gate || !reset);
        if triggered {
            self.state.start_value = self.state.current_value;
            self.state.elapsed_samples = 0;
            self.state.note_off_sample = None;
            self.state.active = true;
        }
        self.state.note_generation = note_generation;
        self.state.reset_generation = reset_generation;
        let released = !gate && self.state.active && self.state.note_off_sample.is_none();
        if released {
            self.state.note_off_sample = Some(self.state.elapsed_samples);
        }
        // 同じ入力ならupdate_sampleが用意した現在値をそのまま使う。
        if self.state.remaining == 0 || reset || triggered || released {
            self.refresh_value();
        }
        self.state.current_value
    }

    /// 設定済み入力を使って次サンプルへ進める。
    pub fn update_sample(&mut self) {
        if self.state.active && self.state.input.is_some() {
            self.state.elapsed_samples = self.state.elapsed_samples.saturating_add(1);
            if self.state.remaining > 1 {
                self.state.remaining -= 1;
                self.state.current_value += self.state.increment;
            } else {
                self.refresh_value();
            }
        }
    }

    pub fn is_active(&self) -> bool {
        self.state.active
    }

    fn refresh_value(&mut self) {
        let Some((parameters, sample_rate, peak)) = self.state.input else {
            return;
        };
        self.state.active &= !self.state.note_off_sample.is_some_and(|off| {
            (self.state.elapsed_samples - off) as f64
                >= (parameters.release_ms * sample_rate / 1000.0).ceil()
        });
        self.state.current_value = if self.state.active {
            value_validated(
                &parameters,
                self.state.elapsed_samples as f64 / sample_rate * 1000.0,
                self.state
                    .note_off_sample
                    .map(|off| off as f64 / sample_rate * 1000.0),
                self.state.start_value,
                peak,
            )
        } else {
            0.0
        };
        if !self.state.active {
            self.state.remaining = u64::MAX;
            return;
        }
        let time = self.state.elapsed_samples as f64 / sample_rate * 1000.0;
        let decay_start = parameters.attack_ms + parameters.hold_ms;
        let decay_end = decay_start + parameters.decay_ms;
        let sample_at = |ms: f64| (ms * sample_rate / 1000.0).ceil() as u64;
        let (slope, boundary) = if let Some(off) = self.state.note_off_sample {
            let off_ms = off as f64 / sample_rate * 1000.0;
            let start = value_validated(&parameters, off_ms, None, self.state.start_value, peak);
            (
                -start / parameters.release_ms,
                off.saturating_add(sample_at(parameters.release_ms)),
            )
        } else if time < parameters.attack_ms {
            (
                (peak - self.state.start_value) / parameters.attack_ms,
                sample_at(parameters.attack_ms),
            )
        } else if time < decay_start {
            (0.0, sample_at(decay_start))
        } else if time < decay_end {
            (
                peak * (parameters.sustain - 1.0) / parameters.decay_ms,
                sample_at(decay_end),
            )
        } else {
            // 正の極小Decayが終了時刻の加算で消えても、境界サンプルのピークを保持し続けない。
            let boundary = if self.state.current_value == peak * parameters.sustain {
                u64::MAX
            } else {
                self.state.elapsed_samples.saturating_add(1)
            };
            (0.0, boundary)
        };
        self.state.increment = slope / sample_rate * 1000.0;
        // ms→sampleとsample→msの丸めが境界の両側へ分かれる場合も、
        // 境界直前から元式で判定し、0ms段階の切替を遅らせない。
        self.state.remaining = boundary
            .saturating_sub(1)
            .saturating_sub(self.state.elapsed_samples);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PARAMETERS: EnvelopeParameters = EnvelopeParameters {
        attack_ms: 100.0,
        hold_ms: 50.0,
        decay_ms: 200.0,
        sustain: 0.4,
        release_ms: 100.0,
    };

    fn assert_value(parameters: &EnvelopeParameters, time: f64, off: Option<f64>, expected: f64) {
        let actual = value(parameters, time, off, 0.0, 1.0);
        assert!(
            (actual - expected).abs() < 1e-12,
            "time={time}, off={off:?}: {actual} != {expected}"
        );
    }

    #[test]
    fn fractional_sample_rate_preserves_zero_decay_boundary() {
        let rate = 44100.5;
        let parameters = EnvelopeParameters {
            attack_ms: 100.0 * 1000.0 / rate,
            hold_ms: 0.0,
            decay_ms: 0.0,
            sustain: 0.31,
            release_ms: 0.0,
        };
        let mut envelope = Envelope::new();
        envelope.set_input(&parameters, rate, 0.7);
        for sample in 0..104 {
            let actual = envelope.generate_sample(true, 1, 0);
            let expected = value(
                &parameters,
                f64::from(sample) / rate * 1000.0,
                None,
                0.0,
                0.7,
            );
            assert!(
                (actual - expected).abs() < 1e-12,
                "sample={sample}: {actual} != {expected}"
            );
            envelope.update_sample();
        }
    }

    #[test]
    fn linear_stages_and_boundaries() {
        for (time, expected) in [
            (0.0, 0.0),
            (25.0, 0.25),
            (50.0, 0.5),
            (100.0, 1.0),
            (125.0, 1.0),
            (150.0, 1.0),
            (200.0, 0.85),
            (250.0, 0.7),
            (350.0, 0.4),
            (10000.0, 0.4),
        ] {
            assert_value(&PARAMETERS, time, None, expected);
            assert_value(&PARAMETERS, time, Some(20000.0), expected);
        }
    }

    #[test]
    fn note_off_releases_from_each_stage_without_jumping() {
        for (off, level) in [
            (0.0, 0.0),
            (50.0, 0.5),
            (100.0, 1.0),
            (125.0, 1.0),
            (150.0, 1.0),
            (250.0, 0.7),
            (350.0, 0.4),
            (1000.0, 0.4),
        ] {
            for (delta, factor) in [
                (0.0, 1.0),
                (25.0, 0.75),
                (50.0, 0.5),
                (100.0, 0.0),
                (10000.0, 0.0),
            ] {
                assert_value(&PARAMETERS, off + delta, Some(off), level * factor);
            }
        }
    }

    #[test]
    fn parameter_endpoints_and_zero_hold() {
        for duration in [1.0, 10000.0] {
            for hold_ms in [0.0, 10000.0] {
                for sustain in [0.0, 1.0] {
                    let p = EnvelopeParameters {
                        attack_ms: duration,
                        hold_ms,
                        decay_ms: duration,
                        sustain,
                        release_ms: duration,
                    };
                    assert!(p.is_valid());
                    assert_value(&p, duration / 2.0, None, 0.5);
                    assert_value(&p, duration + hold_ms, None, 1.0);
                    assert_value(&p, duration * 1.5 + hold_ms, None, (1.0 + sustain) / 2.0);
                    let off = duration * 2.0 + hold_ms;
                    assert_value(&p, off, None, sustain);
                    assert_value(&p, off + duration / 2.0, Some(off), sustain / 2.0);
                    assert_value(&p, off + duration, Some(off), 0.0);
                }
            }
        }
    }

    #[test]
    fn retrigger_uses_full_attack_time_and_zero_stages_finish_immediately() {
        for (start, peak) in [(0.0, 1.0), (0.4, 1.0), (1.0, 1.0), (0.8, 0.2), (0.5, 0.0)] {
            assert_eq!(value(&PARAMETERS, 0.0, None, start, peak), start);
            assert!(
                (value(&PARAMETERS, 50.0, None, start, peak) - (start + peak) / 2.0).abs() < 1e-12
            );
            assert_eq!(value(&PARAMETERS, 100.0, None, start, peak), peak);
            assert!(
                (value(&PARAMETERS, 100.0, Some(50.0), start, peak) - (start + peak) / 4.0).abs()
                    < 1e-12
            );
        }
        let mut p = EnvelopeParameters {
            attack_ms: 0.0,
            ..PARAMETERS
        };
        assert!(p.is_valid());
        assert_eq!(value(&p, 0.0, None, 0.2, 1.0), 1.0);
        p.hold_ms = 0.0;
        p.decay_ms = 0.0;
        assert_eq!(value(&p, 0.0, None, 0.2, 1.0), p.sustain);
        p.release_ms = 0.0;
        assert_eq!(value(&p, 50.0, Some(50.0), 0.2, 1.0), 0.0);
        for invalid in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
            assert_eq!(value(&p, 0.0, None, invalid, 1.0), 0.0);
            assert_eq!(value(&p, 0.0, None, 0.0, invalid), 0.0);
        }
    }

    #[test]
    fn invalid_parameters_and_times_return_zero() {
        for invalid in [-1.0, 10000.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for p in [
                EnvelopeParameters {
                    attack_ms: invalid,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    hold_ms: invalid,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    decay_ms: invalid,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    release_ms: invalid,
                    ..PARAMETERS
                },
            ] {
                assert!(!p.is_valid());
                assert_eq!(value(&p, 50.0, None, 0.0, 1.0), 0.0);
            }
        }
        for sustain in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            let p = EnvelopeParameters {
                sustain,
                ..PARAMETERS
            };
            assert!(!p.is_valid());
            assert_eq!(value(&p, 50.0, None, 0.0, 1.0), 0.0);
        }
        for time in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(value(&PARAMETERS, time, None, 0.0, 1.0), 0.0);
            assert_eq!(value(&PARAMETERS, 50.0, Some(time), 0.0, 1.0), 0.0);
        }
        assert_eq!(
            value(&PARAMETERS, f64::MAX, None, 0.0, 1.0),
            PARAMETERS.sustain
        );
        assert_eq!(value(&PARAMETERS, f64::MAX, Some(50.0), 0.0, 1.0), 0.0);
    }

    #[test]
    fn signed_zero_and_subnormal_durations_have_exact_boundaries() {
        for zero in [0.0, -0.0] {
            let p = EnvelopeParameters {
                attack_ms: zero,
                hold_ms: zero,
                decay_ms: zero,
                sustain: 0.25,
                release_ms: zero,
            };
            for time in [0.0, 1.0] {
                assert_eq!(value(&p, time, None, 0.5, 1.0), 0.25);
                assert_eq!(value(&p, time, Some(0.0), 0.5, 1.0), 0.0);
            }
        }
        let p = EnvelopeParameters {
            attack_ms: f64::from_bits(1),
            ..PARAMETERS
        };
        assert_eq!(value(&p, 0.0, None, 0.25, 1.0), 0.25);
        assert_eq!(value(&p, p.attack_ms, None, 0.25, 1.0), 1.0);
    }

    #[test]
    fn tiny_positive_decay_does_not_hold_the_peak_past_the_boundary() {
        for rate in [1000.0, 44_100.0, 48_000.0, 96_000.0] {
            for decay_ms in [f64::from_bits(1), 1e-14] {
                let p = EnvelopeParameters {
                    decay_ms,
                    ..PARAMETERS
                };
                let mut envelope = Envelope::new();
                envelope.set_input(&p, rate, 1.0);
                for sample in 0..(rate as u32 / 5) {
                    let time = f64::from(sample) / rate * 1000.0;
                    let actual = envelope.generate_sample(true, 1, 0);
                    assert!((actual - value(&p, time, None, 0.0, 1.0)).abs() < 1e-11);
                    envelope.update_sample();
                }
            }
        }
    }

    #[test]
    fn linear_updates_follow_exact_formula_across_fractional_boundaries() {
        for rate in [44_100.0, 96_000.0] {
            for off in [0, 81, 6789] {
                let p = EnvelopeParameters {
                    attack_ms: 10.125,
                    hold_ms: 2.0625,
                    decay_ms: 19.125,
                    sustain: 0.3,
                    release_ms: 43.333,
                };
                let mut envelope = Envelope::new();
                envelope.set_input(&p, rate, 0.8);
                for sample in 0..12_000 {
                    let time = f64::from(sample) / rate * 1000.0;
                    let off_ms = (sample >= off).then_some(f64::from(off) / rate * 1000.0);
                    let expected = value(&p, time, off_ms, 0.0, 0.8);
                    let actual = envelope.generate_sample(sample < off, 1, 0);
                    assert!((actual - expected).abs() < 1e-11);
                    envelope.update_sample();
                }
            }
        }
    }

    #[test]
    fn cached_value_tracks_parameters_rate_peak_and_note_boundaries() {
        let mut envelope = Envelope::new();
        let mut p = EnvelopeParameters {
            attack_ms: 10.0,
            hold_ms: 0.0,
            decay_ms: 10.0,
            sustain: 0.5,
            release_ms: 10.0,
        };
        assert!(envelope.set_input(&p, 1000.0, 1.0));
        assert_eq!(envelope.generate_sample(true, 1, 0), 0.0);
        envelope.update_sample();
        assert_eq!(envelope.generate_sample(true, 1, 0), 0.1);
        p.attack_ms = 20.0;
        envelope.set_input(&p, 1000.0, 1.0);
        assert_eq!(envelope.generate_sample(true, 1, 0), 0.05);
        envelope.update_sample();
        envelope.set_input(&p, 1000.0, 0.5);
        assert_eq!(envelope.generate_sample(true, 1, 0), 0.05);
        envelope.set_input(&p, 2000.0, 0.5);
        assert_eq!(envelope.generate_sample(true, 1, 0), 0.025);
        let invalid = EnvelopeParameters {
            attack_ms: f64::NAN,
            ..p
        };
        assert!(!envelope.set_input(&invalid, 2000.0, 0.5));
        assert_eq!(envelope.generate_sample(false, 2, 1), 0.0);
        envelope.set_input(&p, 2000.0, 0.5);
        assert_eq!(envelope.generate_sample(false, 1, 0), 0.025);
        envelope.update_sample();
        let released = envelope.generate_sample(false, 1, 0);
        assert!((released - 0.02375).abs() < 1e-15);
        envelope.set_input(&p, 2000.0, 0.75);
        assert_eq!(envelope.generate_sample(true, 2, 0), released);
        envelope.update_sample();
        let expected = released * 0.975 + 0.75 * 0.025;
        assert!((envelope.generate_sample(true, 2, 0) - expected).abs() < 1e-15);
        assert_eq!(envelope.generate_sample(false, 2, 1), 0.0);
        assert!(!envelope.is_active());
    }
}

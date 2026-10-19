// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 線形の音量エンベロープ。時刻は呼び出し側からミリ秒単位で与える。

/// エンベロープの時間と音量をまとめたパラメーター。
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeParameters {
    /// 音量が0から1へ上がる時間（1..=5000 ms）。
    pub attack_ms: f64,
    /// 最大音量1を維持する時間（0..=5000 ms）。
    pub hold_ms: f64,
    /// 最大音量1からsustainへ下がる時間（1..=5000 ms）。
    pub decay_ms: f64,
    /// 減衰後に維持する音量（0..=1）。最大音量に対する倍率。
    pub sustain: f64,
    /// 音量が0へ下がる時間（1..=5000 ms）。
    pub release_ms: f64,
}

impl EnvelopeParameters {
    /// 範囲外、NaN、無限大を拒否する。
    pub fn is_valid(&self) -> bool {
        (1.0..=5000.0).contains(&self.attack_ms)
            && (0.0..=5000.0).contains(&self.hold_ms)
            && (1.0..=5000.0).contains(&self.decay_ms)
            && (0.0..=1.0).contains(&self.sustain)
            && (1.0..=5000.0).contains(&self.release_ms)
    }
}

/// ノートオンからelapsed_ms経過した時点の音量倍率（0..=1）を返す。
/// note_off_msもノートオンからの経過時間。Noneならsustainを維持する。
/// 指定されたノートオフ時刻より前は通常の立ち上がり・減衰を計算し、
/// その時刻以降はノートオフ時の音量からrelease_msかけて0へ線形に下げる。
/// パラメーター不正、負の時刻、NaN、無限大では0を返す。
/// 内部状態を持たず、毎回渡されたパラメーターから計算する。
pub fn amplitude(
    parameters: &EnvelopeParameters,
    elapsed_ms: f64,
    note_off_ms: Option<f64>,
) -> f64 {
    if !parameters.is_valid()
        || !elapsed_ms.is_finite()
        || elapsed_ms < 0.0
        || note_off_ms.is_some_and(|time| !time.is_finite() || time < 0.0)
    {
        return 0.0;
    }
    if let Some(off) = note_off_ms.filter(|&off| elapsed_ms >= off) {
        let remaining = (1.0 - (elapsed_ms - off) / parameters.release_ms).max(0.0);
        held_amplitude(parameters, off) * remaining
    } else {
        held_amplitude(parameters, elapsed_ms)
    }
}

/// ノートオフ前のattack → hold → decay → sustainを計算する。
fn held_amplitude(parameters: &EnvelopeParameters, elapsed_ms: f64) -> f64 {
    if elapsed_ms < parameters.attack_ms {
        return elapsed_ms / parameters.attack_ms;
    }
    let after_attack = elapsed_ms - parameters.attack_ms;
    if after_attack < parameters.hold_ms {
        return 1.0;
    }
    let after_hold = after_attack - parameters.hold_ms;
    if after_hold < parameters.decay_ms {
        return 1.0 - (1.0 - parameters.sustain) * (after_hold / parameters.decay_ms);
    }
    parameters.sustain
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

    fn assert_amplitude(
        parameters: &EnvelopeParameters,
        time: f64,
        off: Option<f64>,
        expected: f64,
    ) {
        let actual = amplitude(parameters, time, off);
        assert!(
            (actual - expected).abs() < 1e-12,
            "time={time}, off={off:?}: {actual} != {expected}"
        );
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
            assert_amplitude(&PARAMETERS, time, None, expected);
            assert_amplitude(&PARAMETERS, time, Some(20000.0), expected);
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
                assert_amplitude(&PARAMETERS, off + delta, Some(off), level * factor);
            }
        }
    }

    #[test]
    fn parameter_endpoints_and_zero_hold() {
        for duration in [1.0, 5000.0] {
            for hold_ms in [0.0, 5000.0] {
                for sustain in [0.0, 1.0] {
                    let p = EnvelopeParameters {
                        attack_ms: duration,
                        hold_ms,
                        decay_ms: duration,
                        sustain,
                        release_ms: duration,
                    };
                    assert!(p.is_valid());
                    assert_amplitude(&p, duration / 2.0, None, 0.5);
                    assert_amplitude(&p, duration + hold_ms, None, 1.0);
                    assert_amplitude(&p, duration * 1.5 + hold_ms, None, (1.0 + sustain) / 2.0);
                    let off = duration * 2.0 + hold_ms;
                    assert_amplitude(&p, off, None, sustain);
                    assert_amplitude(&p, off + duration / 2.0, Some(off), sustain / 2.0);
                    assert_amplitude(&p, off + duration, Some(off), 0.0);
                }
            }
        }
    }

    #[test]
    fn invalid_parameters_and_times_are_silent() {
        for value in [-1.0, 5000.1, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            for p in [
                EnvelopeParameters {
                    attack_ms: value,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    hold_ms: value,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    decay_ms: value,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    release_ms: value,
                    ..PARAMETERS
                },
            ] {
                assert!(!p.is_valid());
                assert_eq!(amplitude(&p, 50.0, None), 0.0);
            }
        }
        for value in [0.0, 0.999] {
            for p in [
                EnvelopeParameters {
                    attack_ms: value,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    decay_ms: value,
                    ..PARAMETERS
                },
                EnvelopeParameters {
                    release_ms: value,
                    ..PARAMETERS
                },
            ] {
                assert!(!p.is_valid());
                assert_eq!(amplitude(&p, 50.0, None), 0.0);
            }
        }
        for sustain in [-0.1, 1.1, f64::NAN, f64::INFINITY] {
            let p = EnvelopeParameters {
                sustain,
                ..PARAMETERS
            };
            assert!(!p.is_valid());
            assert_eq!(amplitude(&p, 50.0, None), 0.0);
        }
        for time in [-1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(amplitude(&PARAMETERS, time, None), 0.0);
            assert_eq!(amplitude(&PARAMETERS, 50.0, Some(time)), 0.0);
        }
        assert_eq!(amplitude(&PARAMETERS, f64::MAX, None), PARAMETERS.sustain);
        assert_eq!(amplitude(&PARAMETERS, f64::MAX, Some(50.0)), 0.0);
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! ホストのAPIから独立した、位相を保持する単一のサイン波オシレータ。

#[derive(Clone, Copy)]
pub struct OscillatorParams {
    pub frequency_hz: f64,
    pub amplitude: f64,
}

pub struct Oscillator {
    params: OscillatorParams,
    // 次に生成するサンプルの位相。単位は周期、範囲は[0, 1)。
    phase: f64,
}

impl Oscillator {
    pub fn new() -> Self {
        Self {
            params: OscillatorParams {
                frequency_hz: 440.0,
                amplitude: 0.0,
            },
            phase: 0.0,
        }
    }

    // パラメーターだけを変更する。再発音・音程変更で位相を初期化しない。
    pub fn set_params(&mut self, params: OscillatorParams) -> bool {
        if !params.frequency_hz.is_finite()
            || params.frequency_hz < 0.0
            || !(0.0..=1.0).contains(&params.amplitude)
        {
            return false;
        }
        self.params = params;
        true
    }

    pub fn next_sample(&mut self, sample_rate: f64) -> f64 {
        if !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }
        let sample = self.params.amplitude * (std::f64::consts::TAU * self.phase).sin();
        // 小さなサンプルレートでも除算のオーバーフローを起こさない。
        let increment = (self.params.frequency_hz % sample_rate) / sample_rate;
        self.phase = (self.phase + increment).fract();
        sample
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parameter_updates_and_sample_rate_changes_preserve_phase() {
        let mut oscillator = Oscillator::new();
        assert!(oscillator.set_params(OscillatorParams {
            frequency_hz: 1.0,
            amplitude: 0.5
        }));
        assert_eq!(oscillator.next_sample(4.0), 0.0);
        assert!((oscillator.next_sample(4.0) - 0.5).abs() < 1e-14);
        assert!(oscillator.set_params(OscillatorParams {
            frequency_hz: 2.0,
            amplitude: 0.25
        }));
        assert!(oscillator.next_sample(8.0).abs() < 1e-14);
        assert!((oscillator.next_sample(8.0) + 0.25).abs() < 1e-14);
        assert_eq!(oscillator.next_sample(8.0), 0.0);
    }

    #[test]
    fn rejects_invalid_parameters_and_never_emits_nonfinite_samples() {
        let mut oscillator = Oscillator::new();
        for frequency_hz in [f64::NAN, f64::INFINITY, -1.0] {
            assert!(!oscillator.set_params(OscillatorParams {
                frequency_hz,
                amplitude: 0.5
            }));
        }
        for amplitude in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            assert!(!oscillator.set_params(OscillatorParams {
                frequency_hz: 440.0,
                amplitude
            }));
        }
        assert!(oscillator.set_params(OscillatorParams {
            frequency_hz: f64::MAX,
            amplitude: 1.0
        }));
        for rate in [
            0.0,
            -1.0,
            f64::NAN,
            f64::INFINITY,
            f64::MIN_POSITIVE,
            44100.0,
            96000.0,
        ] {
            for _ in 0..16 {
                assert!(oscillator.next_sample(rate).is_finite());
            }
        }
    }
}

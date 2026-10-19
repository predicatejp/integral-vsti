// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器回路の定常入力と内部状態を分離した単一のサイン波オシレータ。

/// イベントではなく、サンプルごとに参照する発振器回路の定常入力。
/// 周波数・振幅は目標値。現段階ではエンベロープを持たず、そのまま使用する。
#[derive(Clone, Copy)]
pub struct OscillatorInput {
    /// 目標周波数（Hz）。有限の0以上の値。入力を変更しても位相は初期化しない。
    pub frequency_hz: f64,
    /// 目標ピーク振幅（0..=1）。0でもgateがtrueなら位相は進む。
    pub amplitude: f64,
    /// 発音を指示する定常ゲート。falseなら消音し、位相を保持する。
    pub gate: bool,
}

impl OscillatorInput {
    pub fn is_valid(&self) -> bool {
        self.frequency_hz.is_finite()
            && self.frequency_hz >= 0.0
            && (0.0..=1.0).contains(&self.amplitude)
    }
}

/// 発振器回路の内部状態。入力値のコピーは保持しない。
/// 将来エンベロープを実装する場合、その現在値もこの状態に含める。
pub struct OscillatorState {
    /// 次に生成するサンプルの位相（周期単位、0以上1未満）。
    /// 初期値は0。入力変更や再発音でも保持し、ゲートが閉じている間は進めない。
    phase: f64,
}

impl OscillatorState {
    pub fn new() -> Self {
        Self { phase: 0.0 }
    }

    /// 現在の入力・状態でサンプルを生成し、その後で次のサンプルに向けて状態を更新する。
    /// sample_rateは処理環境のサンプルレート（Hz）。不正な入力・レートでは無音を返し、状態を保持する。
    pub fn next_sample(&mut self, input: &OscillatorInput, sample_rate: f64) -> f64 {
        if !input.is_valid() || !sample_rate.is_finite() || sample_rate <= 0.0 {
            return 0.0;
        }
        let sample = self.generate_sample(input);
        self.update(input, sample_rate);
        sample
    }

    /// 現在の状態を読み取るだけで、位相を進めずにサンプルを生成する。
    fn generate_sample(&self, input: &OscillatorInput) -> f64 {
        if input.gate {
            input.amplitude * (std::f64::consts::TAU * self.phase).sin()
        } else {
            0.0
        }
    }

    /// サンプル生成後に、定常入力を用いて内部状態を1サンプル分進める。
    fn update(&mut self, input: &OscillatorInput, sample_rate: f64) {
        if input.gate {
            // 小さなサンプルレートでも除算のオーバーフローを起こさない。
            let increment = (input.frequency_hz % sample_rate) / sample_rate;
            self.phase = (self.phase + increment).fract();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steady_input_changes_preserve_state_and_apply_to_the_current_sample() {
        let mut state = OscillatorState::new();
        let mut input = OscillatorInput {
            frequency_hz: 1.0,
            amplitude: 0.5,
            gate: true,
        };
        assert_eq!(state.next_sample(&input, 4.0), 0.0); // 更新前の位相0で生成
        assert!((state.next_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        // 入力だけを変更する。保持していた位相1/2から、新しい振幅・周波数で処理する。
        input.frequency_hz = 2.0;
        input.amplitude = 0.25;
        assert!(state.next_sample(&input, 8.0).abs() < 1e-14);
        input.gate = false;
        for _ in 0..7 {
            assert_eq!(state.next_sample(&input, 8.0), 0.0);
        }
        input.gate = true;
        assert!((state.next_sample(&input, 8.0) + 0.25).abs() < 1e-14);
        // 振幅0はゲート閉とは異なり、状態更新を続ける。
        input.amplitude = 0.0;
        assert_eq!(state.next_sample(&input, 8.0), 0.0);
        input.amplitude = 0.5;
        assert!((state.next_sample(&input, 8.0) - 0.5).abs() < 1e-14);
    }

    #[test]
    fn invalid_inputs_preserve_state_and_never_emit_nonfinite_samples() {
        let mut state = OscillatorState::new();
        let input = OscillatorInput {
            frequency_hz: 1.0,
            amplitude: 0.5,
            gate: true,
        };
        state.next_sample(&input, 4.0); // 位相1/4を保持しておく。
        for frequency_hz in [f64::NAN, f64::INFINITY, -1.0] {
            let invalid = OscillatorInput {
                frequency_hz,
                ..input
            };
            assert!(!invalid.is_valid());
            assert_eq!(state.next_sample(&invalid, 4.0), 0.0);
        }
        for amplitude in [f64::NAN, f64::INFINITY, -0.1, 1.1] {
            let invalid = OscillatorInput { amplitude, ..input };
            assert!(!invalid.is_valid());
            assert_eq!(state.next_sample(&invalid, 4.0), 0.0);
        }
        for rate in [0.0, -1.0, f64::NAN, f64::INFINITY] {
            assert_eq!(state.next_sample(&input, rate), 0.0);
        }
        assert!((state.next_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        let extreme = OscillatorInput {
            frequency_hz: f64::MAX,
            amplitude: 1.0,
            gate: true,
        };
        for rate in [f64::MIN_POSITIVE, 44100.0, 96000.0] {
            for _ in 0..16 {
                assert!(state.next_sample(&extreme, rate).is_finite());
            }
        }
    }
}

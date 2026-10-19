// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 発振器回路の定常入力と内部状態を分離した単一のオシレータ。

/// 対称な三角波・ノコギリ波の実効値をピーク振幅へ変換する倍率（√3）。
const SQRT_3: f64 = 1.732_050_807_568_877_2;

/// 位相から生成する波形。矩形波はデューティ比50%、ノコギリ波は右上がり。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "波形は内部入力で指定する。ホストからの選択は未実装"
    )
)]
pub enum Waveform {
    /// 位相0から正方向に立ち上がるサイン波。
    Sine,
    /// 位相0で0、1/4で正のピーク、3/4で負のピークとなる三角波。
    Triangle,
    /// 前半周期が正、後半周期が負の50%デューティ矩形波。
    Square,
    /// 位相0の負のピークから直線的に上昇し、周期境界で折り返すノコギリ波。
    Sawtooth,
}

impl Waveform {
    /// 理想波形の1周期における実効値をピーク振幅へ変換する倍率。
    fn peak_per_rms(self) -> f64 {
        match self {
            Self::Sine => std::f64::consts::SQRT_2,
            Self::Triangle | Self::Sawtooth => SQRT_3,
            Self::Square => 1.0,
        }
    }

    /// 位相[0, 1)からピーク振幅1の波形を生成する。帯域制限は行わない。
    fn unit_sample(self, phase: f64) -> f64 {
        match self {
            Self::Sine => (std::f64::consts::TAU * phase).sin(),
            Self::Triangle => 1.0 - 4.0 * ((phase + 0.25).fract() - 0.5).abs(),
            Self::Square => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            Self::Sawtooth => 2.0 * phase - 1.0,
        }
    }
}

/// イベントではなく、サンプルごとに参照する発振器回路の定常入力。
/// 周波数・振幅は目標値。現段階ではエンベロープを持たず、そのまま使用する。
#[derive(Clone, Copy)]
pub struct OscillatorInput {
    /// 生成する波形。変更しても内部の位相を初期化しない。
    pub waveform: Waveform,
    /// 目標周波数（Hz）。有限の0以上の値。入力を変更しても位相は初期化しない。
    pub frequency_hz: f64,
    /// 理想波形の目標実効値（0..=1）。ピーク値は波形に依存する。0でもgateがtrueなら位相は進む。
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
            input.amplitude * input.waveform.peak_per_rms() * input.waveform.unit_sample(self.phase)
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
    fn waveforms_have_expected_shape_and_peak() {
        let cases = [
            (
                Waveform::Sine,
                std::f64::consts::FRAC_1_SQRT_2,
                [
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    1.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                    -std::f64::consts::FRAC_1_SQRT_2,
                    -1.0,
                    -std::f64::consts::FRAC_1_SQRT_2,
                ],
            ),
            (
                Waveform::Triangle,
                0.866_025_403_784_438_6,
                [0.0, 0.5, 1.0, 0.5, 0.0, -0.5, -1.0, -0.5],
            ),
            (
                Waveform::Square,
                0.5,
                [1.0, 1.0, 1.0, 1.0, -1.0, -1.0, -1.0, -1.0],
            ),
            (
                Waveform::Sawtooth,
                0.866_025_403_784_438_6,
                [-1.0, -0.75, -0.5, -0.25, 0.0, 0.25, 0.5, 0.75],
            ),
        ];
        for (waveform, peak, expected) in cases {
            let mut state = OscillatorState::new();
            let input = OscillatorInput {
                waveform,
                frequency_hz: 1.0,
                amplitude: 0.5,
                gate: true,
            };
            for _ in 0..2 {
                for unit in expected {
                    assert!(
                        (state.next_sample(&input, 8.0) - peak * unit).abs() < 1e-14,
                        "{waveform:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn waveform_changes_and_gate_preserve_phase() {
        let mut state = OscillatorState::new();
        let mut input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5,
            gate: true,
        };
        assert_eq!(state.next_sample(&input, 4.0), 0.0);
        input.waveform = Waveform::Triangle;
        assert!((state.next_sample(&input, 4.0) - 0.866_025_403_784_438_6).abs() < 1e-14);
        input.waveform = Waveform::Square;
        assert_eq!(state.next_sample(&input, 4.0), -0.5);
        input.gate = false;
        input.waveform = Waveform::Sawtooth;
        assert_eq!(state.next_sample(&input, 4.0), 0.0);
        input.gate = true;
        assert!((state.next_sample(&input, 4.0) - 0.433_012_701_892_219_3).abs() < 1e-14);
    }

    #[test]
    fn steady_input_changes_preserve_state_and_apply_to_the_current_sample() {
        let mut state = OscillatorState::new();
        let mut input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5 / std::f64::consts::SQRT_2,
            gate: true,
        };
        assert_eq!(state.next_sample(&input, 4.0), 0.0); // 更新前の位相0で生成
        assert!((state.next_sample(&input, 4.0) - 0.5).abs() < 1e-14);
        // 入力だけを変更する。保持していた位相1/2から、新しい振幅・周波数で処理する。
        input.frequency_hz = 2.0;
        input.amplitude = 0.25 / std::f64::consts::SQRT_2;
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
        input.amplitude = 0.5 / std::f64::consts::SQRT_2;
        assert!((state.next_sample(&input, 8.0) - 0.5).abs() < 1e-14);
    }

    #[test]
    fn invalid_inputs_preserve_state_and_never_emit_nonfinite_samples() {
        let mut state = OscillatorState::new();
        let input = OscillatorInput {
            waveform: Waveform::Sine,
            frequency_hz: 1.0,
            amplitude: 0.5 / std::f64::consts::SQRT_2,
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
            waveform: Waveform::Sine,
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

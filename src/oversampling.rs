// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Voice合成後の2相補間と、エフェクタ・EQ・Limiter後の共有FIR間引き。
//! 129 taps: cutoff=(20000/44100 + 0.5)/2 (host Fs単位), Kaiser β=4.6。
//! 17 taps: cutoff=host Fs (input=4Fs), Kaiser β=5.65。係数和は1。
//! 下記は対称係数の前半（中心を含む）。補間時だけ利得2を適用する。

type Stereo = [f64; 2];
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    X1,
    X2,
    X4,
}
impl Mode {
    pub fn factor(self) -> usize {
        match self {
            Self::X1 => 1,
            Self::X2 => 2,
            Self::X4 => 4,
        }
    }
}

#[cfg_attr(test, derive(Clone))]
struct HistoryState<const N: usize> {
    /// 二重に保持し、折り返す履歴も連続したNサンプルとして参照する。
    samples: [[Stereo; N]; 2],
    /// 次の書き込み位置。書き込み後は最古サンプルの位置になる。
    position: usize,
}
#[cfg_attr(test, derive(Clone))]
pub(crate) struct History<const N: usize> {
    state: HistoryState<N>,
}
impl<const N: usize> History<N> {
    pub(crate) fn new() -> Self {
        Self {
            state: HistoryState {
                samples: [[[0.0; 2]; N]; 2],
                position: 0,
            },
        }
    }
    pub(crate) fn reset(&mut self) {
        self.state.samples = [[[0.0; 2]; N]; 2];
        self.state.position = 0;
    }
    #[inline]
    fn push_sample(&mut self, sample: Stereo) {
        let s = &mut self.state;
        s.samples[0][s.position] = sample;
        s.samples[1][s.position] = sample;
        s.position += 1;
        if s.position == N {
            s.position = 0;
        }
    }
    #[inline]
    fn samples(&self) -> &[Stereo] {
        &self.state.samples.as_flattened()[self.state.position..self.state.position + N]
    }
    #[inline]
    fn filter_sample(&self, coefficients: &[f64], step: usize) -> Stereo {
        let x = self.samples();
        let mut out = x[N / 2].map(|v| v * coefficients[N / 2]);
        for i in (step - 1..N / 2).step_by(step) {
            for ch in 0..2 {
                out[ch] += (x[i][ch] + x[N - 1 - i][ch]) * coefficients[i];
            }
        }
        out
    }
    /// ゼロ挿入を省いた2相FIR。位相ごとの対称係数を利用する。
    #[inline]
    fn interpolate_samples(&self, coefficients: &[f64], halfband: bool) -> [Stereo; 2] {
        let x = self.samples();
        std::array::from_fn(|phase| {
            if halfband && phase == 0 {
                return x[N / 2].map(|v| v * (2.0 * coefficients[N - 1]));
            }
            let len = N - phase;
            let mut out = [0.0; 2];
            for j in 0..len / 2 {
                for ch in 0..2 {
                    out[ch] += (x[phase + j][ch] + x[N - 1 - j][ch]) * coefficients[phase + 2 * j];
                }
            }
            if len % 2 == 1 {
                for ch in 0..2 {
                    out[ch] += x[phase + len / 2][ch] * coefficients[phase + 2 * (len / 2)];
                }
            }
            out.map(|v| 2.0 * v)
        })
    }
}

#[cfg_attr(test, derive(Clone))]
struct UpsamplerState {
    /// Fs→2Fsの129タップ補間に必要な入力履歴。
    wide: History<65>,
    /// 2Fs→4Fsの17タップ補間に必要な入力履歴。
    half: History<9>,
}
/// 合成済みステレオ信号を補間する。処理中のメモリ確保を行わない。
#[cfg_attr(test, derive(Clone))]
pub struct Upsampler {
    state: UpsamplerState,
}
impl Upsampler {
    pub fn new() -> Self {
        Self {
            state: UpsamplerState {
                wide: History::new(),
                half: History::new(),
            },
        }
    }
    pub fn reset(&mut self) {
        self.state.wide.reset();
        self.state.half.reset();
    }
    pub fn process_samples(&mut self, input: Stereo, mode: Mode) -> [Stereo; 4] {
        let mut out = [[0.0; 2]; 4];
        if mode == Mode::X1 {
            out[0] = input;
            return out;
        }
        self.state.wide.push_sample(input);
        let pair = self.state.wide.interpolate_samples(&WIDE, false);
        if mode == Mode::X2 {
            out[..2].copy_from_slice(&pair);
        } else {
            for (i, sample) in pair.into_iter().enumerate() {
                self.state.half.push_sample(sample);
                out[i * 2..i * 2 + 2]
                    .copy_from_slice(&self.state.half.interpolate_samples(&HALF, true));
            }
        }
        out
    }
}

#[cfg_attr(test, derive(Clone))]
struct DownsamplerState {
    /// 2Fs→Fsの129タップFIR履歴。
    wide: History<129>,
    /// 4Fs→2Fsの17タップFIR履歴。
    half: History<17>,
}
/// エフェクタ系列の最終段で使う間引き器。倍率間を揃える追加遅延は持たない。
#[cfg_attr(test, derive(Clone))]
pub struct Downsampler {
    state: DownsamplerState,
}
impl Downsampler {
    pub fn new() -> Self {
        Self {
            state: DownsamplerState {
                wide: History::new(),
                half: History::new(),
            },
        }
    }
    pub fn reset(&mut self) {
        self.state.wide.reset();
        self.state.half.reset();
    }
    /// phaseは合算した内部遅延のfactor剰余。対応する位相で間引いて端数を吸収する。
    pub fn process_sample(&mut self, input: [Stereo; 4], mode: Mode, phase: usize) -> Stereo {
        debug_assert!(phase < mode.factor());
        let s = &mut self.state;
        let mut output = [0.0; 2];
        match mode {
            Mode::X1 => input[0],
            Mode::X2 => {
                for (i, sample) in input[..2].iter().enumerate() {
                    s.wide.push_sample(*sample);
                    if i == phase {
                        output = s.wide.filter_sample(&WIDE, 1);
                    }
                }
                output
            }
            Mode::X4 => {
                for (i, sample) in input.into_iter().enumerate() {
                    s.half.push_sample(sample);
                    if i & 1 == phase & 1 {
                        s.wide.push_sample(s.half.filter_sample(&HALF, 2));
                        if i == phase {
                            output = s.wide.filter_sample(&WIDE, 1);
                        }
                    }
                }
                output
            }
        }
    }
}

/// 補間・間引きの往復遅延（ホストサンプル単位）。
pub fn latency_samples(mode: Mode) -> usize {
    match mode {
        Mode::X1 => 0,
        Mode::X2 => 64,
        Mode::X4 => 68,
    }
}

const WIDE: [f64; 65] = [
    0.00026024079185969133,
    3.4684654293607665e-05,
    -0.0003574998702221878,
    -0.00010789669611870991,
    0.0004599294702770341,
    0.00021854475932266493,
    -0.0005599825228542519,
    -0.0003719156524457658,
    0.0006480156810176474,
    0.000571840536787308,
    -0.0007124015237054525,
    -0.0008201366663879026,
    0.0007397659883458194,
    0.0011160623923926734,
    -0.0007153525936289717,
    -0.0014558134757132598,
    0.0006235079543805441,
    0.0018320879755469727,
    -0.0004482757870278595,
    -0.0022337441598575506,
    0.00017407947406177603,
    0.002645570868961619,
    0.00021353326975751203,
    -0.0030481824380311283,
    -0.0007271155893393262,
    0.0034180405169375034,
    0.0013765525990465826,
    -0.0037275926849457733,
    -0.002168336850944026,
    0.003945502162952038,
    0.003104927033813866,
    -0.004036923186563861,
    -0.00418422713058786,
    0.003963750749267774,
    0.0053992195287926835,
    -0.0036847376354238203,
    -0.006737779865284914,
    0.00315531858081899,
    0.008182693919277891,
    -0.002326896722574895,
    -0.00971188798166257,
    0.0011452027586429212,
    0.011298874251916873,
    0.0004529263468944554,
    -0.012913402445095508,
    -0.0025455119615009204,
    0.014522298472686527,
    0.005236352589424819,
    -0.01609046134798417,
    -0.00867541531597579,
    0.017581980896008684,
    0.013098726691513405,
    -0.0189613319069536,
    -0.01891232282652943,
    0.02019459546876946,
    0.026888434890021604,
    -0.021250655653064197,
    -0.03869788566480545,
    0.022102319690258665,
    0.05871939081462163,
    -0.022727312300866588,
    -0.10308355332577569,
    0.023109099856157733,
    0.31723433827011543,
    0.4766522837538542,
];

const HALF: [f64; 9] = [
    0.0,
    -0.0036726047503355236,
    0.0,
    0.02103492955318969,
    0.0,
    -0.07312662748771219,
    0.0,
    0.30577186189073474,
    0.4999848815882464,
];

#[cfg(test)]
mod tests {
    use super::*;

    fn kernel(mode: Mode) -> Vec<f64> {
        let wide: Vec<_> = (0..129).map(|i| WIDE[i.min(128 - i)]).collect();
        if mode == Mode::X2 {
            return wide;
        }
        let mut out = vec![0.0; 273];
        for i in 0..17 {
            for (j, h) in wide.iter().enumerate() {
                out[i + 2 * j] += HALF[i.min(16 - i)] * h;
            }
        }
        out
    }

    #[test]
    fn every_decimation_phase_matches_direct_convolution() {
        for mode in [Mode::X2, Mode::X4] {
            let r = mode.factor();
            let h = kernel(mode);
            let input: Vec<Stereo> = (0..180)
                .map(|i| {
                    let x = ((i * 37 % 101) as f64 - 50.0) / 50.0;
                    [x, x * -0.3]
                })
                .collect();
            let mut high = vec![[0.0; 2]; input.len() * r];
            for (t, output) in high.iter_mut().enumerate() {
                for (k, h) in h.iter().enumerate().take(t + 1) {
                    if (t - k) % r == 0 {
                        for ch in 0..2 {
                            output[ch] += r as f64 * h * input[(t - k) / r][ch];
                        }
                    }
                }
                *output = output.map(|x| (x * 3.0).clamp(-1.0, 1.0));
            }
            for phase in 0..r {
                let mut up = Upsampler::new();
                let mut down = Downsampler::new();
                for (n, &input) in input.iter().enumerate() {
                    let mut internal = up.process_samples(input, mode);
                    for sample in &mut internal[..r] {
                        *sample = sample.map(|x| (x * 3.0).clamp(-1.0, 1.0));
                    }
                    let actual = down.process_sample(internal, mode, phase);
                    let t = n * r + phase;
                    for ch in 0..2 {
                        let expected: f64 = h
                            .iter()
                            .enumerate()
                            .take(t + 1)
                            .map(|(k, h)| h * high[t - k][ch])
                            .sum();
                        assert!(
                            (actual[ch] - expected).abs() < 1e-10,
                            "{mode:?} phase={phase} n={n}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn combined_delay_uses_decimation_phase_without_padding() {
        for mode in [Mode::X1, Mode::X2, Mode::X4] {
            let r = mode.factor();
            for delay in 0..12 {
                let mut up = Upsampler::new();
                let mut down = Downsampler::new();
                let mut history = std::collections::VecDeque::from(vec![[0.0; 2]; delay]);
                let mut output = Vec::new();
                for n in 0..320 {
                    let x = f64::from(n == 0);
                    let mut internal = up.process_samples([x, -x], mode);
                    for sample in &mut internal[..r] {
                        history.push_back(*sample);
                        *sample = history.pop_front().unwrap();
                    }
                    let y = down.process_sample(internal, mode, delay % r);
                    assert_eq!(y[0], -y[1]);
                    output.push(y[0]);
                }
                let peak = output
                    .iter()
                    .enumerate()
                    .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                    .unwrap()
                    .0;
                assert_eq!(peak, latency_samples(mode) + delay / r);
                assert!((output.iter().sum::<f64>() - 1.0).abs() < 1e-6);
                up.reset();
                down.reset();
                for _ in 0..320 {
                    assert_eq!(
                        down.process_sample(up.process_samples([0.0; 2], mode), mode, delay % r),
                        [0.0; 2]
                    );
                }
            }
        }
    }
}

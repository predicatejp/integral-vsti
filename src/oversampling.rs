// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! ディストーション前の2相補間と、Voice加算後の共有FIR間引き。
//! 129 taps: cutoff=(20000/44100 + 0.5)/2 (host Fs単位), Kaiser β=4.6。
//! 17 taps: cutoff=host Fs (input=4Fs), Kaiser β=5.65。係数和は1。
//! 下記は対称係数の前半（中心を含む）。補間時だけ利得2を適用する。

type Stereo = [f64; 2];
/// x1の純遅延、x2の32+32+4、x4の32+2+2+32を揃える。
pub const LATENCY: usize = 68;

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

struct HistoryState<const N: usize> {
    /// 二重に保持し、折り返す履歴も連続したNサンプルとして参照する。
    samples: [[Stereo; N]; 2],
    /// 次の書き込み位置。書き込み後は最古サンプルの位置になる。
    position: usize,
}
struct History<const N: usize> {
    state: HistoryState<N>,
}
impl<const N: usize> History<N> {
    fn new() -> Self {
        Self {
            state: HistoryState {
                samples: [[[0.0; 2]; N]; 2],
                position: 0,
            },
        }
    }
    fn reset(&mut self) {
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
    fn delay_sample(&mut self, input: Stereo) -> Stereo {
        let out = self.state.samples[0][self.state.position];
        self.push_sample(input);
        out
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

struct UpsamplerState {
    /// Fs→2Fsの129タップ補間に必要な入力履歴。
    wide: History<65>,
    /// 2Fs→4Fsの17タップ補間に必要な入力履歴。
    half: History<9>,
}
/// Voiceごとに所有する補間器。処理中のメモリ確保を行わない。
pub struct Upsampler {
    state: Box<UpsamplerState>,
}
impl Upsampler {
    pub fn new() -> Self {
        Self {
            state: Box::new(UpsamplerState {
                wide: History::new(),
                half: History::new(),
            }),
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

struct DownsamplerState {
    /// 2Fs→Fsの129タップFIR履歴。
    wide: History<129>,
    /// 4Fs→2Fsの17タップFIR履歴。
    half: History<17>,
    /// x1を他倍率の往復遅延へ揃える純遅延。
    delay1: History<LATENCY>,
    /// x2の往復64サンプルへ追加する純遅延。
    delay2: History<4>,
}
/// 全Voiceの内部サンプルを加算した後に1個だけ使う間引き器。
pub struct Downsampler {
    state: Box<DownsamplerState>,
}
impl Downsampler {
    pub fn new() -> Self {
        Self {
            state: Box::new(DownsamplerState {
                wide: History::new(),
                half: History::new(),
                delay1: History::new(),
                delay2: History::new(),
            }),
        }
    }
    pub fn reset(&mut self) {
        self.state.wide.reset();
        self.state.half.reset();
        self.state.delay1.reset();
        self.state.delay2.reset();
    }
    pub fn process_sample(&mut self, input: [Stereo; 4], mode: Mode) -> Stereo {
        let s = &mut self.state;
        match mode {
            Mode::X1 => s.delay1.delay_sample(input[0]),
            Mode::X2 => {
                s.wide.push_sample(input[0]);
                let out = s.wide.filter_sample(&WIDE, 1);
                s.wide.push_sample(input[1]);
                s.delay2.delay_sample(out)
            }
            Mode::X4 => {
                s.half.push_sample(input[0]);
                s.wide.push_sample(s.half.filter_sample(&HALF, 2));
                let out = s.wide.filter_sample(&WIDE, 1);
                s.half.push_sample(input[1]);
                s.half.push_sample(input[2]);
                s.wide.push_sample(s.half.filter_sample(&HALF, 2));
                s.half.push_sample(input[3]);
                out
            }
        }
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
    use crate::distortion::{self, ClipperType, DistortionParameters};

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
    fn polyphase_matches_zero_insertion_and_direct_convolution() {
        for mode in [Mode::X2, Mode::X4] {
            let r = mode.factor();
            let h = kernel(mode);
            for clipper in [ClipperType::None, ClipperType::Soft, ClipperType::Hard] {
                let input: Vec<Stereo> = (0..160)
                    .map(|i| {
                        let v = ((i * 37 % 101) as f64 - 50.0) / 50.0;
                        [v, -0.3 * v]
                    })
                    .collect();
                let p: Vec<_> = (0..input.len() * r)
                    .map(|i| DistortionParameters {
                        clipper,
                        gain: 1.0 + i as f64 / 200.0,
                        mix: 0.25 + i as f64 / (input.len() * r) as f64 * 0.75,
                    })
                    .collect();
                let mut wet = vec![[0.0; 2]; input.len() * r];
                for t in 0..wet.len() {
                    let mut up = [0.0; 2];
                    for (k, h) in h.iter().enumerate().take(t + 1) {
                        if (t - k) % r == 0 {
                            for ch in 0..2 {
                                up[ch] += r as f64 * h * input[(t - k) / r][ch];
                            }
                        }
                    }
                    wet[t] = distortion::process_sample(up, &p[t]);
                }
                let mut up = Upsampler::new();
                let mut down = Downsampler::new();
                for (n, input) in input.into_iter().enumerate() {
                    let mut samples = up.process_samples(input, mode);
                    for j in 0..r {
                        samples[j] = distortion::process_sample(samples[j], &p[n * r + j]);
                    }
                    let actual = down.process_sample(samples, mode);
                    let padding = if mode == Mode::X2 { 4 } else { 0 };
                    for ch in 0..2 {
                        let expected = if n < padding {
                            0.0
                        } else {
                            let t = (n - padding) * r;
                            h.iter()
                                .enumerate()
                                .take(t + 1)
                                .map(|(k, h)| h * wet[t - k][ch])
                                .sum::<f64>()
                        };
                        assert!(
                            (actual[ch] - expected).abs() < 2e-13,
                            "{mode:?} {clipper:?} sample={n}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn latency_dc_and_reset_match_all_modes() {
        for mode in [Mode::X1, Mode::X2, Mode::X4] {
            let mut up = Upsampler::new();
            let mut down = Downsampler::new();
            let mut peak = (0, 0.0_f64);
            for n in 0..160 {
                let x = f64::from(n == 0);
                let out = down.process_sample(up.process_samples([x, -x], mode), mode);
                assert_eq!(out[0], -out[1]);
                if out[0].abs() > peak.1 {
                    peak = (n, out[0].abs());
                }
            }
            assert_eq!(peak.0, LATENCY);
            for _ in 0..200 {
                down.process_sample(up.process_samples([0.2, -0.3], mode), mode);
            }
            let dc = down.process_sample(up.process_samples([0.2, -0.3], mode), mode);
            assert!((dc[0] - 0.2).abs() < 1e-6 && (dc[1] + 0.3).abs() < 1e-6);
            up.reset();
            down.reset();
            for _ in 0..160 {
                assert_eq!(
                    down.process_sample(up.process_samples([0.0; 2], mode), mode),
                    [0.0; 2]
                );
            }
        }
    }
}

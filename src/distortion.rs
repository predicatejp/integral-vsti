// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! フィルタ出力のプリゲイン・クリップ・原音との線形混合。履歴は持たない。

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipperType {
    None,
    Soft,
    Hard,
}

#[derive(Clone, Copy)]
pub struct DistortionParameters {
    /// NoneはGainとMixも含めたバイパス。
    pub clipper: ClipperType,
    /// 処理音のクリップ前に掛ける線形倍率。dBからの換算は設定側で行う。
    pub gain: f64,
    /// 処理音の混合率（0〜1）。原音側にはGainを掛けない。
    pub mix: f64,
}

/// 検証済みの設定を使い、同時刻の左右サンプルを独立に処理する。
#[inline]
pub fn process_sample(input: [f64; 2], parameters: &DistortionParameters) -> [f64; 2] {
    if parameters.clipper == ClipperType::None || parameters.mix == 0.0 {
        return input;
    }
    let wet = match parameters.clipper {
        ClipperType::None => input,
        ClipperType::Soft => input.map(|x| (x * parameters.gain).tanh()),
        ClipperType::Hard => input.map(|x| (x * parameters.gain).clamp(-1.0, 1.0)),
    };
    std::array::from_fn(|ch| input[ch] * (1.0 - parameters.mix) + wet[ch] * parameters.mix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bypass_clipping_and_dry_wet_endpoints() {
        for gain in [1.0, 2.0, 10.0_f64.powf(48.0 / 20.0)] {
            for x in [-4.0_f64, -1.0, -0.25, 0.0, 0.25, 1.0, 4.0] {
                for clipper in [ClipperType::None, ClipperType::Soft, ClipperType::Hard] {
                    for mix in [0.0, 0.25, 1.0] {
                        let p = DistortionParameters { clipper, gain, mix };
                        let actual = process_sample([x, -x], &p);
                        let wet = match clipper {
                            ClipperType::None => x,
                            ClipperType::Soft => (gain * x).tanh(),
                            ClipperType::Hard => (gain * x).clamp(-1.0, 1.0),
                        };
                        let expected = x * (1.0 - mix) + wet * mix;
                        assert!((actual[0] - expected).abs() < 1e-14);
                        assert_eq!(actual[0], -actual[1]);
                        if clipper == ClipperType::None || mix == 0.0 {
                            assert_eq!(actual, [x, -x]);
                        }
                        if clipper != ClipperType::None && mix == 1.0 {
                            assert!(actual[0].abs() <= 1.0);
                        }
                    }
                }
            }
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 音源パラメーターの共通定義・値変換・共有値。

use crate::distortion::{ClipperType, DistortionParameters};
use crate::envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use crate::filter::FilterType;
use crate::oscillator::{PhaseMode, Waveform};
use std::sync::atomic::{AtomicU64, Ordering};

#[path = "parameters/automation.rs"]
pub mod automation;
#[path = "parameters/state.rs"]
pub mod state;

pub const COUNT: usize = 134;
pub const PORTAMENTO: usize = 132;
pub const PITCH: usize = 133;
/// ホストのピッチベンド受信値。音色の保存対象とGUIから独立させる。
pub const PITCH_BEND_ID: u32 = 0x4245_4e44;
pub const PITCH_BEND_INDEX: usize = COUNT + 1;
pub const HOST_COUNT: usize = COUNT + 2;
/// 各スロットの種類、Clipper/Gain/Mix、Delayの音価/Feedback/Mix/Stereo、ReverbのDecay/Damping/Size/Mix、ChorusのTime/Depth/Rate/Detune/Feedback/Mix、PhaserのCenter/Depth/Rate/Feedback/Mix、CompressorのThreshold/Ratio/Attack/Release/Knee/Makeup/Mix。IDは位置に固定する。
pub const EFFECT_SLOTS: [[usize; 30]; 3] = [
    [
        42, 31, 32, 33, 51, 52, 53, 60, 63, 64, 65, 66, 75, 76, 77, 78, 79, 80, 93, 94, 95, 96, 97,
        109, 110, 111, 112, 113, 114, 115,
    ],
    [
        43, 45, 46, 47, 54, 55, 56, 61, 67, 68, 69, 70, 81, 82, 83, 84, 85, 86, 98, 99, 100, 101,
        102, 116, 117, 118, 119, 120, 121, 122,
    ],
    [
        44, 48, 49, 50, 57, 58, 59, 62, 71, 72, 73, 74, 87, 88, 89, 90, 91, 92, 103, 104, 105, 106,
        107, 123, 124, 125, 126, 127, 128, 129,
    ],
];
pub const EFFECT_TYPES: [crate::effects::EffectType; 7] = [
    crate::effects::EffectType::Off,
    crate::effects::EffectType::Chorus,
    crate::effects::EffectType::Compressor,
    crate::effects::EffectType::Delay,
    crate::effects::EffectType::Distortion,
    crate::effects::EffectType::Phaser,
    crate::effects::EffectType::Reverb,
];
pub const DELAY_STEREO_MODES: [crate::delay::StereoMode; 4] = [
    crate::delay::StereoMode::Stereo,
    crate::delay::StereoMode::Cross,
    crate::delay::StereoMode::PingPongLeft,
    crate::delay::StereoMode::PingPongRight,
];
pub const DELAY_BEATS: [&str; 14] = [
    "1/32", "1/16 T", "1/16", "1/8 T", "1/16 D", "1/8", "1/4 T", "1/8 D", "1/4", "1/2 T", "1/4 D",
    "1/2", "1/2 D", "1/1",
];

/// 同じ種類の設定は全スロットで同じ単位・量子化を用いる。
pub const fn control_kind(index: usize) -> usize {
    match index {
        PORTAMENTO => 1, // AHDSRと同じ時間軸。
        45 | 48 => 31,
        46 | 49 => 32,
        47 | 50 => 33,
        42..=44 => 42,
        54 | 57 => 51,
        55 | 58 => 52,
        53 | 56 | 59 => 33,
        61 | 62 => 60,
        67 | 71 => 63,
        68 | 72 => 64,
        69 | 73 => 65,
        66 | 70 | 74 => 33,
        81 | 87 => 75,
        82 | 88 => 76,
        83 | 89 => 77,
        84 | 90 => 78,
        85 | 91 => 79,
        80 | 86 | 92 => 33,
        98 | 103 => 93,
        99 | 104 => 94,
        95 | 100 | 105 => 77,
        96 | 101 | 106 => 79,
        97 | 102 | 107 => 33,
        109..=129 => match (index - 109) % 7 {
            2 => 1,  // Attack: 既存の時間軸を共用する。
            3 => 5,  // Release
            6 => 33, // Mix
            kind => 109 + kind,
        },
        _ => index,
    }
}

/// 1回の比較で得た変更位置を、Synthと各エフェクトへ共有する。
pub(crate) struct ParameterChanges([u64; COUNT.div_ceil(64)]);

impl ParameterChanges {
    #[inline]
    pub fn new(values: &[f64; COUNT], previous: &[f64; COUNT]) -> Self {
        use std::arch::x86_64::*;
        let mut words = [0; COUNT.div_ceil(64)];
        for (word, output) in words.iter_mut().enumerate().take(COUNT / 64) {
            for group in 0..4 {
                let offset = word * 64 + group * 16;
                // 対応するx86_64ではSSE2を常備する。offset..offset+16は両配列内。
                // !=比較はNaNを変更、+0/-0を同値とし、符号付きpackは-1/0を保持する。
                let bits = unsafe {
                    let pairs = std::array::from_fn::<_, 8, _>(|pair| {
                        _mm_cmpneq_pd(
                            _mm_loadu_pd(values.as_ptr().add(offset + pair * 2)),
                            _mm_loadu_pd(previous.as_ptr().add(offset + pair * 2)),
                        )
                    });
                    let quarters = std::array::from_fn::<_, 4, _>(|i| {
                        _mm_castps_si128(_mm_shuffle_ps::<0x88>(
                            _mm_castpd_ps(pairs[2 * i]),
                            _mm_castpd_ps(pairs[2 * i + 1]),
                        ))
                    });
                    _mm_movemask_epi8(_mm_packs_epi16(
                        _mm_packs_epi32(quarters[0], quarters[1]),
                        _mm_packs_epi32(quarters[2], quarters[3]),
                    ))
                };
                *output |= (bits as u64) << (group * 16);
            }
        }
        for index in COUNT / 64 * 64..COUNT {
            words[index / 64] |= u64::from(values[index] != previous[index]) << (index % 64);
        }
        Self(words)
    }

    /// 呼出し元が更新した候補だけ比較する。候補外の値は既存値と同じであること。
    #[inline]
    pub fn from_candidates(
        values: &[f64; COUNT],
        previous: &[f64; COUNT],
        candidates: [u64; COUNT.div_ceil(64)],
    ) -> Self {
        let mut words = [0; COUNT.div_ceil(64)];
        for (word, mut pending) in candidates.into_iter().enumerate() {
            while pending != 0 {
                let bit = pending.trailing_zeros() as usize;
                let index = word * 64 + bit;
                pending &= pending - 1;
                words[word] |= u64::from(values[index] != previous[index]) << bit;
            }
        }
        Self(words)
    }

    /// DSPの保存値を更新するための、64項目ずつの数値変更mask。
    #[inline]
    pub fn words(&self) -> [u64; COUNT.div_ceil(64)] {
        self.0
    }

    #[inline]
    pub fn contains(&self, index: usize) -> bool {
        self.0[index / 64] & (1 << (index % 64)) != 0
    }

    #[inline]
    pub fn any(&self, first: usize, end: usize) -> bool {
        let first_word = first / 64;
        let last_word = (end - 1) / 64;
        let low = u64::MAX << (first % 64);
        let high = u64::MAX >> (63 - (end - 1) % 64);
        if first_word == last_word {
            self.0[first_word] & low & high != 0
        } else {
            (self.0[first_word] & low | self.0[last_word] & high) != 0
                || self.0[first_word + 1..last_word]
                    .iter()
                    .any(|word| *word != 0)
        }
    }
}

/// 隣接スロットの種類と全設定を交換する。ホストのIDは移動しない。
pub fn swap_effects(values: &mut [f64; COUNT], first: usize) {
    for (a, b) in EFFECT_SLOTS[first].into_iter().zip(EFFECT_SLOTS[first + 1]) {
        values.swap(a, b);
    }
}
/// Osc1・Osc2・Osc3のVolume設定位置。
pub const OSCILLATOR_VOLUMES: [usize; 3] = [22, 25, 28];

pub fn is_volume(index: usize) -> bool {
    matches!(index, 13 | 22 | 25 | 28)
}

pub fn is_waveform(index: usize) -> bool {
    matches!(index, 0 | 23 | 26)
}
/// 音色設定と別に公開するプログラム選択ID（PROG）。
pub const PROGRAM_ID: u32 = 0x5052_4f47;
/// 変更キュー配列では音色設定の末尾にProgramを置く。保存する値には含めない。
pub const PROGRAM_INDEX: usize = COUNT;
pub const PROGRAM_NAME: &str = "Sine";

/// 内蔵Sineの全設定。編集値から独立した固定の音色定義。
pub fn sine_preset() -> [f64; COUNT] {
    initial_values(
        EnvelopeParameters {
            attack_ms: 0.0,
            hold_ms: 0.0,
            decay_ms: 0.0,
            sustain: 1.0,
            release_ms: 0.0,
        },
        FilterType::None,
    )
}

pub const CLIPPER_TYPES: [ClipperType; 2] = [ClipperType::Soft, ClipperType::Hard];
pub const CLIPPER_NAMES: [&str; 2] = ["Soft", "Hard"];

pub const PHASE_MODES: [PhaseMode; 3] = [PhaseMode::Keep, PhaseMode::Zero, PhaseMode::Random];
pub const PHASE_NAMES: [&str; 3] = ["Keep", "Zero", "Random"];

/// ホストのオートメーションとの対応を維持する固定ID（"WAVE"）。
pub const WAVEFORM_ID: u32 = 0x5741_5645;
/// 値・保存形式・表示順の対応。既存の番号を変更しない。
pub const WAVEFORMS: [Waveform; 5] = [
    Waveform::Sine,
    Waveform::Triangle,
    Waveform::Square,
    Waveform::Sawtooth,
    Waveform::Noise,
];
pub const WAVEFORM_NAMES: [&str; 5] = ["Sine", "Triangle", "Square", "Sawtooth", "Noise"];
/// フィルタ設定の順序。状態保存・オートメーションとの対応を固定する。
pub const FILTER_TYPES: [FilterType; 7] = [
    FilterType::None,
    FilterType::LowPass1,
    FilterType::LowPass2,
    FilterType::HighPass1,
    FilterType::HighPass2,
    FilterType::BandPass1,
    FilterType::BandPass2,
];
pub const FILTER_NAMES: [&str; 7] = [
    "Off",
    "LowPass1",
    "LowPass2",
    "HighPass1",
    "HighPass2",
    "BandPass1",
    "BandPass2",
];

// 同じ行から従来と同じ配列を生成し、ID検索のメモリ配置も維持する。
macro_rules! define_parameters {
    ($($id:expr, $name:literal, $default:expr;)*) => {
        pub const IDS: [u32; COUNT] = [$($id),*];
        pub const NAMES: [&str; COUNT] = [$($name),*];
        const DEFAULT_PLAIN: [f64; COUNT] = [$($default),*];
    };
}

define_parameters! {
    WAVEFORM_ID, "Osc1 Waveform", 0.0;
    0x4154544b, "Attack", DEFAULT_ENVELOPE.attack_ms;
    0x484f4c44, "Hold", DEFAULT_ENVELOPE.hold_ms;
    0x44454359, "Decay", DEFAULT_ENVELOPE.decay_ms;
    0x53555354, "Sustain", DEFAULT_ENVELOPE.sustain;
    0x52454c53, "Release", DEFAULT_ENVELOPE.release_ms;
    0x554e4953, "Unison", 1.0;
    0x4454554e, "Detune", 0.0;
    0x504f4c59, "Polyphony", 1.0;
    0x46545950, "Filter", FilterType::LowPass1 as u32 as f64;
    0x46435554, "Cutoff", 4800.0;
    0x46524553, "Resonance", 1.0;
    0x50484153, "Phase", 0.0;
    0x4d564f4c, "Volume", 0.0;
    0x43415454, "Cutoff Attack", DEFAULT_ENVELOPE.attack_ms;
    0x43484c44, "Cutoff Hold", DEFAULT_ENVELOPE.hold_ms;
    0x43444543, "Cutoff Decay", DEFAULT_ENVELOPE.decay_ms;
    0x43535553, "Cutoff Sustain", DEFAULT_ENVELOPE.sustain;
    0x4352454c, "Cutoff Release", DEFAULT_ENVELOPE.release_ms;
    0x43414d54, "Cutoff Amount", 0.0;
    0x57494454, "Width", 0.0;
    0x4d50414e, "Pan", 0.0;
    0x4f31564c, "Osc1 Volume", 0.0;
    0x4f325746, "Osc2 Waveform", 0.0;
    0x4f32544e, "Osc2 Tune", 0.0;
    0x4f32564c, "Osc2 Volume", f64::NEG_INFINITY;
    0x4f335746, "Osc3 Waveform", 0.0;
    0x4f33544e, "Osc3 Tune", 0.0;
    0x4f33564c, "Osc3 Volume", f64::NEG_INFINITY;
    0x4f32464d, "Osc2 Mod", 0.0;
    0x4f33464d, "Osc3 Mod", 0.0;
    0x43545950, "Effect 1 Clipper", 0.0;
    0x43474149, "Effect 1 Gain", 0.0;
    0x434d4958, "Effect 1 Mix", 100.0;
    0x4551_0000, "EQ Enable", 0.0;
    0x4551_0001, "EQ Low Frequency", 250.0;
    0x4551_0002, "EQ Low Gain", 0.0;
    0x4551_0003, "EQ Mid Frequency", 1000.0;
    0x4551_0004, "EQ Mid Gain", 0.0;
    0x4551_0005, "EQ Mid Q", 1.0;
    0x4551_0006, "EQ High Frequency", 4000.0;
    0x4551_0007, "EQ High Gain", 0.0;
    0x4546_3154, "Effect 1", 0.0;
    0x4546_3254, "Effect 2", 0.0;
    0x4546_3354, "Effect 3", 0.0;
    0x4546_3243, "Effect 2 Clipper", 0.0;
    0x4546_3247, "Effect 2 Gain", 0.0;
    0x4546_324d, "Effect 2 Mix", 100.0;
    0x4546_3343, "Effect 3 Clipper", 0.0;
    0x4546_3347, "Effect 3 Gain", 0.0;
    0x4546_334d, "Effect 3 Mix", 100.0;
    0x4546_3142, "Effect 1 Delay Time", 5.0;
    0x4546_3146, "Effect 1 Feedback", 35.0;
    0x4546_3144, "Effect 1 Delay Mix", 25.0;
    0x4546_3242, "Effect 2 Delay Time", 5.0;
    0x4546_3246, "Effect 2 Feedback", 35.0;
    0x4546_3244, "Effect 2 Delay Mix", 25.0;
    0x4546_3342, "Effect 3 Delay Time", 5.0;
    0x4546_3346, "Effect 3 Feedback", 35.0;
    0x4546_3344, "Effect 3 Delay Mix", 25.0;
    0x4546_3153, "Effect 1 Stereo", 0.0;
    0x4546_3253, "Effect 2 Stereo", 0.0;
    0x4546_3353, "Effect 3 Stereo", 0.0;
    0x52314443, "Effect 1 Reverb Decay", 2.0;
    0x5231444d, "Effect 1 Reverb Damping", 6000.0;
    0x5231535a, "Effect 1 Reverb Size", 100.0;
    0x52314d58, "Effect 1 Reverb Mix", 25.0;
    0x52324443, "Effect 2 Reverb Decay", 2.0;
    0x5232444d, "Effect 2 Reverb Damping", 6000.0;
    0x5232535a, "Effect 2 Reverb Size", 100.0;
    0x52324d58, "Effect 2 Reverb Mix", 25.0;
    0x52334443, "Effect 3 Reverb Decay", 2.0;
    0x5233444d, "Effect 3 Reverb Damping", 6000.0;
    0x5233535a, "Effect 3 Reverb Size", 100.0;
    0x52334d58, "Effect 3 Reverb Mix", 25.0;
    0x4331544d, "Effect 1 Chorus Time", 20.0;
    0x43314450, "Effect 1 Chorus Depth", 2.0;
    0x43315254, "Effect 1 Chorus Rate", 0.7;
    0x43314454, "Effect 1 Chorus Detune", 0.0;
    0x43314642, "Effect 1 Chorus Feedback", 0.0;
    0x43314d58, "Effect 1 Chorus Mix", 50.0;
    0x4332544d, "Effect 2 Chorus Time", 20.0;
    0x43324450, "Effect 2 Chorus Depth", 2.0;
    0x43325254, "Effect 2 Chorus Rate", 0.7;
    0x43324454, "Effect 2 Chorus Detune", 0.0;
    0x43324642, "Effect 2 Chorus Feedback", 0.0;
    0x43324d58, "Effect 2 Chorus Mix", 50.0;
    0x4333544d, "Effect 3 Chorus Time", 20.0;
    0x43334450, "Effect 3 Chorus Depth", 2.0;
    0x43335254, "Effect 3 Chorus Rate", 0.7;
    0x43334454, "Effect 3 Chorus Detune", 0.0;
    0x43334642, "Effect 3 Chorus Feedback", 0.0;
    0x43334d58, "Effect 3 Chorus Mix", 50.0;
    0x50314346, "Effect 1 Phaser Center", 1000.0;
    0x50314450, "Effect 1 Phaser Depth", 1200.0;
    0x50315254, "Effect 1 Phaser Rate", 0.5;
    0x50314642, "Effect 1 Phaser Feedback", 50.0;
    0x50314d58, "Effect 1 Phaser Mix", 50.0;
    0x50324346, "Effect 2 Phaser Center", 1000.0;
    0x50324450, "Effect 2 Phaser Depth", 1200.0;
    0x50325254, "Effect 2 Phaser Rate", 0.5;
    0x50324642, "Effect 2 Phaser Feedback", 50.0;
    0x50324d58, "Effect 2 Phaser Mix", 50.0;
    0x50334346, "Effect 3 Phaser Center", 1000.0;
    0x50334450, "Effect 3 Phaser Depth", 1200.0;
    0x50335254, "Effect 3 Phaser Rate", 0.5;
    0x50334642, "Effect 3 Phaser Feedback", 50.0;
    0x50334d58, "Effect 3 Phaser Mix", 50.0;
    0x4c490000, "Limiter", 0.0;
    0x43500100, "Effect 1 Compressor Threshold", -18.0;
    0x43500101, "Effect 1 Compressor Ratio", 4.0;
    0x43500102, "Effect 1 Compressor Attack", 10.0;
    0x43500103, "Effect 1 Compressor Release", 100.0;
    0x43500104, "Effect 1 Compressor Knee", 6.0;
    0x43500105, "Effect 1 Compressor Makeup", 0.0;
    0x43500106, "Effect 1 Compressor Mix", 100.0;
    0x43500200, "Effect 2 Compressor Threshold", -18.0;
    0x43500201, "Effect 2 Compressor Ratio", 4.0;
    0x43500202, "Effect 2 Compressor Attack", 10.0;
    0x43500203, "Effect 2 Compressor Release", 100.0;
    0x43500204, "Effect 2 Compressor Knee", 6.0;
    0x43500205, "Effect 2 Compressor Makeup", 0.0;
    0x43500206, "Effect 2 Compressor Mix", 100.0;
    0x43500300, "Effect 3 Compressor Threshold", -18.0;
    0x43500301, "Effect 3 Compressor Ratio", 4.0;
    0x43500302, "Effect 3 Compressor Attack", 10.0;
    0x43500303, "Effect 3 Compressor Release", 100.0;
    0x43500304, "Effect 3 Compressor Knee", 6.0;
    0x43500305, "Effect 3 Compressor Makeup", 0.0;
    0x43500306, "Effect 3 Compressor Mix", 100.0;
    0x4153_454e, "Amp Sensitivity", 100.0;
    0x4653_454e, "Filter Amount Sensitivity", 0.0;
    0x504f_5254, "Portamento", 0.0;
    0x5049_5443, "Pitch", 200.0;
}

/// 離散リストの表示順。連続値や数値ノブでは空のスライスを返す。
pub fn choices(index: usize) -> &'static [&'static str] {
    if (42..=44).contains(&index) {
        return &[
            "Off",
            "Chorus",
            "Compressor",
            "Delay",
            "Distortion",
            "Phaser",
            "Reverb",
        ];
    }
    match control_kind(index) {
        0 | 23 | 26 => &WAVEFORM_NAMES,
        9 => &FILTER_NAMES,
        12 => &PHASE_NAMES,
        31 => &CLIPPER_NAMES,
        34 | 108 => &["Off", "On"],
        51 => &DELAY_BEATS,
        60 => &["Dual", "Cross", "PP-L", "PP-R"],
        _ => &[],
    }
}

/// ノブの整数tick上限。選択リストはchoicesの列挙番号を使う。
pub fn max_tick(index: usize) -> u32 {
    let index = control_kind(index);
    if is_time(index) {
        TIME_TICKS
    } else {
        match index {
            31 | 34 | 108 => 1,
            51 => 13,
            42 => 6,
            109 => 600,
            110 => 190,
            113 | 114 => 240,
            93 => 400,
            94 => 240,
            60 => 3,
            75 => 399,
            76 => 1000,
            77 => 495,
            78 => 200,
            79 => 190,
            63 => 200,
            64 => 300,
            65 => 150,
            52 => 100,
            35 | 37 | 40 => 300,
            36 | 38 | 41 => 360,
            39 => 200,
            6 => 6,
            8 => 31,
            19 => 960,
            21 | 29 | 30 => 200,
            10 => 1200,
            13 | 22 | 25 | 28 => 720,
            24 | 27 | 32 => 480,
            11 => 950,
            PITCH => 120,
            _ => 100,
        }
    }
}

/// 対数ノブで正確に選ぶ実値。目盛に加え、250/4000/6000は初期値用の換算点。
fn knob_log_points(index: usize) -> &'static [(u32, f64)] {
    match control_kind(index) {
        35 | 37 | 40 => &[
            (0, 20.0),
            (40, 50.0),
            (70, 100.0),
            (100, 200.0),
            (110, 250.0),
            (140, 500.0),
            (170, 1000.0),
            (200, 2000.0),
            (230, 4000.0),
            (240, 5000.0),
            (270, 10000.0),
            (300, 20000.0),
        ],
        64 => &[
            (0, 500.0),
            (56, 1000.0),
            (113, 2000.0),
            (187, 5000.0),
            (202, 6000.0),
            (244, 10000.0),
            (300, 20000.0),
        ],
        93 => &[
            (0, 100.0),
            (75, 200.0),
            (175, 500.0),
            (250, 1000.0),
            (325, 2000.0),
            (400, 4000.0),
        ],
        39 | 63 => &[
            (0, 0.1),
            (30, 0.2),
            (70, 0.5),
            (100, 1.0),
            (130, 2.0),
            (170, 5.0),
            (200, 10.0),
        ],
        _ => &[],
    }
}

/// ホストの正規化値をGUIの整数tickへ換算する。表示更新だけでは再通知しない。
pub fn to_tick(index: usize, value: f64) -> u32 {
    let points = knob_log_points(index);
    if !points.is_empty() {
        let plain = to_plain(index, value);
        for pair in points.windows(2) {
            let [(start, low), (end, high)] = [pair[0], pair[1]];
            if plain <= high {
                let fraction = (plain / low).ln() / (high / low).ln();
                return (f64::from(start) + f64::from(end - start) * fraction).round() as u32;
            }
        }
    }
    (value * f64::from(max_tick(index))).round() as u32
}

/// GUIとホストへ公開する実単位。
pub fn unit(index: usize) -> &'static str {
    let index = control_kind(index);
    if is_time(index) {
        "ms"
    } else {
        match index {
            35 | 37 | 40 | 64 | 77 | 93 => "Hz",
            63 => "s",
            75 | 76 => "ms",
            65 => "%",
            36 | 38 | 41 | 109 | 113 | 114 => "dB",
            13 | 22 | 25 | 28 | 32 => "dB",
            20 | 21 | 29 | 30 | 33 | 52 | 79 | 130 | 131 => "%",
            7 | 10 | 19 | 24 | 27 | 94 | PITCH => "cent",
            _ => "",
        }
    }
}

/// 時間軸の上端。0ms専用端点と、1ms〜10sの400区間。
const TIME_TICKS: u32 = 401;
pub fn index(id: u32) -> Option<usize> {
    IDS.iter().position(|&candidate| candidate == id)
}
pub fn is_time(index: usize) -> bool {
    matches!(control_kind(index), 1 | 2 | 3 | 5 | 14 | 15 | 16 | 18)
}
/// 変更点まで直前の値を保持するパラメーター。Cutoffは連続値だが補間しない。
pub fn holds_between_points(index: usize) -> bool {
    matches!(
        index,
        0 | 6 | 8 | 9 | 10 | 12 | 23 | 26 | 31 | 34 | 42..=45 | 48 | 51 | 54 | 57 | 60..=62 | 65 | 69 | 73 | 108 | PROGRAM_INDEX
    )
}
/// GUIの整数tickを実単位へ換算する。時間0..=401、sustain/detune/width 0..=100、unison 0..=6、polyphony 0..=31、cutoff 0..=1200、resonance 0..=950、volume 0..=720（0は−∞）、cutoff amount 0..=960、pan 0..=200、tune 0..=480。
pub fn tick_to_plain(index: usize, tick: u32) -> f64 {
    let index = control_kind(index);
    let points = knob_log_points(index);
    // 各表はtick=0から始まる。対応点では補間せず、定義した実値を返す。
    for (position, &(end, high)) in points.iter().enumerate() {
        if tick == end {
            return high;
        }
        if tick < end {
            let (start, low) = points[position - 1];
            let fraction = f64::from(tick - start) / f64::from(end - start);
            return low * (high / low).powf(fraction);
        }
    }
    if index >= 34 {
        return to_plain(index, f64::from(tick) / f64::from(max_tick(index)));
    }
    if is_volume(index) {
        if tick == 0 {
            f64::NEG_INFINITY
        } else {
            -60.0 + f64::from(tick) / 10.0
        }
    } else if is_time(index) {
        if tick == 0 {
            0.0
        } else {
            10.0_f64.powf((f64::from(tick) - 1.0) / 100.0).min(10000.0)
        }
    } else {
        match index {
            6 | 8 => f64::from(tick) + 1.0,
            7 | 9 | 20 | 29 | 30 | 31 | 33 => f64::from(tick),
            32 => f64::from(tick) / 10.0,
            21 => f64::from(tick) - 100.0,
            10 => f64::from(tick) * 10.0 - 2400.0,
            11 => 0.5 + f64::from(tick) / 100.0,
            24 | 27 => f64::from(tick) * 10.0 - 2400.0,
            19 => f64::from(tick) * 10.0 - 4800.0,
            _ => f64::from(tick) / 100.0,
        }
    }
}
pub fn to_plain(index: usize, normalized: f64) -> f64 {
    control_to_plain(control_kind(index), normalized)
}

/// 対象コントロールが既知のDSPでは、種別の検索をコンパイル時に済ませる。
#[inline]
pub fn plain_for<const INDEX: usize>(normalized: f64) -> f64 {
    control_to_plain(const { control_kind(INDEX) }, normalized)
}

#[inline]
fn control_to_plain(index: usize, normalized: f64) -> f64 {
    if is_volume(index) {
        return if normalized == 0.0 {
            f64::NEG_INFINITY
        } else {
            normalized * 72.0 - 60.0
        };
    }
    if is_waveform(index) {
        return from_normalized(normalized).map_or(0.0, |w| f64::from(waveform_index(w)));
    }
    match index {
        31 | 34 | 108 => return (normalized * 2.0).floor().min(1.0),
        51 => return (normalized * 14.0).floor().min(13.0),
        42 => return (normalized * 7.0).floor().min(6.0),
        109 => return normalized * 60.0 - 60.0,
        110 => return 1.0 + normalized * 19.0,
        113 | 114 => return normalized * 24.0,
        93 => return 100.0 * 40_f64.powf(normalized),
        94 => return 2400.0 * normalized,
        PITCH => return 1200.0 * normalized,
        PITCH_BEND_INDEX => return normalized * 2.0 - 1.0,
        60 => return (normalized * 4.0).floor().min(3.0),
        75 => return 0.1 + normalized * 39.9,
        76 => return normalized * 10.0,
        77 => return 0.05 + normalized * 4.95,
        78 => return normalized * 0.2 - 0.1,
        79 => return normalized * 190.0 - 95.0,
        63 => return 0.1 * 100_f64.powf(normalized),
        64 => return 500.0 * 40_f64.powf(normalized),
        65 => return 50.0 + 150.0 * normalized,
        52 => return normalized * 100.0,
        35 | 37 | 40 => return 20.0 * 1000_f64.powf(normalized),
        36 | 38 | 41 => return normalized * 36.0 - 18.0,
        39 => return 0.1 * 100_f64.powf(normalized),
        4 | 17 => return normalized,
        6 => return (normalized * 7.0).floor().min(6.0) + 1.0,
        7 | 20 | 33 | 130 | 131 => return normalized * 100.0,
        32 => return normalized * 48.0,
        29 | 30 => return normalized * 200.0,
        21 => return normalized * 200.0 - 100.0,
        8 => return (normalized * 32.0).floor().min(31.0) + 1.0,
        9 => return (normalized * 7.0).floor().min(6.0),
        10 => return normalized * 12000.0 - 2400.0,
        11 => return 0.5 + normalized * 9.5,
        12 => return (normalized * 3.0).floor().min(2.0),
        24 | 27 => return normalized * 4800.0 - 2400.0,
        19 => return normalized * 9600.0 - 4800.0,
        _ => {}
    }
    if normalized == 1.0 {
        return 10000.0;
    }
    let tick = normalized * f64::from(TIME_TICKS);
    if tick <= 1.0 {
        tick
    } else {
        10.0_f64.powf((tick - 1.0) / 100.0).min(10000.0)
    }
}
pub fn to_normalized(index: usize, plain: f64) -> f64 {
    let index = control_kind(index);
    if is_volume(index) {
        return if plain == f64::NEG_INFINITY {
            0.0
        } else {
            (plain + 60.0) / 72.0
        };
    }
    if is_waveform(index) {
        return plain.clamp(0.0, 4.0).round() / 4.0;
    }
    match index {
        31 | 34 | 108 => return plain.clamp(0.0, 1.0).round(),
        51 => return plain.clamp(0.0, 13.0).round() / 13.0,
        42 => return plain.clamp(0.0, 6.0).round() / 6.0,
        109 => return (plain + 60.0) / 60.0,
        110 => return (plain - 1.0) / 19.0,
        113 | 114 => return plain / 24.0,
        93 => return (plain / 100.0).log(40.0),
        94 => return plain / 2400.0,
        PITCH => return plain / 1200.0,
        PITCH_BEND_INDEX => return (plain + 1.0) / 2.0,
        60 => return plain.clamp(0.0, 3.0).round() / 3.0,
        75 => return (plain - 0.1) / 39.9,
        76 => return plain / 10.0,
        77 => return (plain - 0.05) / 4.95,
        78 => return (plain + 0.1) / 0.2,
        79 => return (plain + 95.0) / 190.0,
        63 => return (plain / 0.1).log(100.0),
        64 => return (plain / 500.0).log(40.0),
        65 => return (plain - 50.0) / 150.0,
        52 => return plain / 100.0,
        35 | 37 | 40 => return (plain / 20.0).log(1000.0),
        36 | 38 | 41 => return (plain + 18.0) / 36.0,
        39 => return (plain / 0.1).log(100.0),
        4 | 17 => return plain,
        6 => return (plain.clamp(1.0, 7.0).round() - 1.0) / 6.0,
        7 | 20 | 33 | 130 | 131 => return plain / 100.0,
        32 => return plain / 48.0,
        29 | 30 => return plain / 200.0,
        21 => return (plain + 100.0) / 200.0,
        8 => return (plain.clamp(1.0, 32.0).round() - 1.0) / 31.0,
        9 => return plain.clamp(0.0, 6.0).round() / 6.0,
        10 => return (plain + 2400.0) / 12000.0,
        11 => return (plain - 0.5) / 9.5,
        12 => return plain.clamp(0.0, 2.0).round() / 2.0,
        24 | 27 => return (plain + 2400.0) / 4800.0,
        19 => return (plain + 4800.0) / 9600.0,
        _ => {}
    }
    if plain == 10000.0 {
        return 1.0;
    }
    (if plain <= 1.0 {
        plain
    } else {
        1.0 + 100.0 * plain.log10()
    }) / f64::from(TIME_TICKS)
}
pub fn valid_plain(index: usize, plain: f64) -> bool {
    let index = control_kind(index);
    if is_volume(index) {
        return plain == f64::NEG_INFINITY || (-60.0..=12.0).contains(&plain);
    }
    match index {
        0 | 23 | 26 => (0.0..=4.0).contains(&plain) && plain.fract() == 0.0,
        31 | 34 | 108 => (0.0..=1.0).contains(&plain) && plain.fract() == 0.0,
        51 => (0.0..=13.0).contains(&plain) && plain.fract() == 0.0,
        42 => (0.0..=6.0).contains(&plain) && plain.fract() == 0.0,
        109 => (-60.0..=0.0).contains(&plain),
        110 => (1.0..=20.0).contains(&plain),
        113 | 114 => (0.0..=24.0).contains(&plain),
        93 => (100.0..=4000.0).contains(&plain),
        94 => (0.0..=2400.0).contains(&plain),
        PITCH => (0.0..=1200.0).contains(&plain),
        PITCH_BEND_INDEX => (-1.0..=1.0).contains(&plain),
        60 => (0.0..=3.0).contains(&plain) && plain.fract() == 0.0,
        75 => (0.1..=40.0).contains(&plain),
        76 => (0.0..=10.0).contains(&plain),
        77 => (0.05..=5.0).contains(&plain),
        78 => (-0.1..=0.1).contains(&plain),
        79 => (-95.0..=95.0).contains(&plain),
        63 => (0.1..=10.0).contains(&plain),
        64 => (500.0..=20000.0).contains(&plain),
        65 => (50.0..=200.0).contains(&plain),
        52 => (0.0..=100.0).contains(&plain),
        35 | 37 | 40 => (20.0..=20000.0).contains(&plain),
        36 | 38 | 41 => (-18.0..=18.0).contains(&plain),
        39 => (0.1..=10.0).contains(&plain),
        4 | 17 => (0.0..=1.0).contains(&plain),
        6 => (1.0..=7.0).contains(&plain) && plain.fract() == 0.0,
        7 | 20 | 33 | 130 | 131 => (0.0..=100.0).contains(&plain),
        29 | 30 => (0.0..=200.0).contains(&plain),
        21 => (-100.0..=100.0).contains(&plain),
        8 => (1.0..=32.0).contains(&plain) && plain.fract() == 0.0,
        9 => (0.0..=6.0).contains(&plain) && plain.fract() == 0.0,
        10 => (-2400.0..=9600.0).contains(&plain),
        11 => (0.5..=10.0).contains(&plain),
        12 => (0.0..=2.0).contains(&plain) && plain.fract() == 0.0,
        24 | 27 => (-2400.0..=2400.0).contains(&plain),
        19 => (-4800.0..=4800.0).contains(&plain),
        32 => (0.0..=48.0).contains(&plain),
        _ => (0.0..=10000.0).contains(&plain),
    }
}
pub fn canonical(index: usize, value: f64) -> f64 {
    if index == PROGRAM_INDEX {
        0.0 // 1音色の一覧は全正規化値を番号0へ対応させる。
    } else if is_waveform(index) {
        normalized(from_normalized(value).expect("validated normalized value"))
    } else if matches!(
        control_kind(index),
        6 | 8 | 9 | 12 | 31 | 34 | 42 | 51 | 60 | 108
    ) {
        to_normalized(index, to_plain(index, value))
    } else {
        value
    }
}
pub fn defaults() -> [f64; COUNT] {
    initial_values(DEFAULT_ENVELOPE, FilterType::LowPass1)
}

/// 初期音色とSineに共通する固定値。編集済みのパラメーターは参照しない。
fn initial_values(p: EnvelopeParameters, filter: FilterType) -> [f64; COUNT] {
    let mut plain = DEFAULT_PLAIN;
    let envelope = [p.attack_ms, p.hold_ms, p.decay_ms, p.sustain, p.release_ms];
    plain[1..6].copy_from_slice(&envelope);
    plain[14..19].copy_from_slice(&envelope);
    plain[9] = f64::from(filter as u32);
    std::array::from_fn(|i| to_normalized(i, plain[i]))
}
pub fn eq(values: &[f64; COUNT]) -> crate::eq::EqParameters {
    crate::eq::EqParameters {
        enabled: to_plain(34, values[34]) != 0.0,
        frequencies_hz: [35, 37, 40].map(|i| to_plain(i, values[i])),
        gains_db: [36, 38, 41].map(|i| to_plain(i, values[i])),
        mid_q: to_plain(39, values[39]),
    }
}

pub fn distortion(values: &[f64; COUNT], slot: usize) -> DistortionParameters {
    let [_, clipper, gain, mix, ..] = EFFECT_SLOTS[slot];
    DistortionParameters {
        clipper: CLIPPER_TYPES[plain_for::<31>(values[clipper]) as usize],
        gain: 10.0_f64.powf(plain_for::<32>(values[gain]) / 20.0),
        mix: values[mix],
    }
}

pub fn delay(values: &[f64; COUNT], slot: usize) -> crate::delay::DelayParameters {
    let [_, _, _, _, beat, feedback, mix, stereo, ..] = EFFECT_SLOTS[slot];
    crate::delay::DelayParameters {
        division: plain_for::<51>(values[beat]) as usize,
        stereo: DELAY_STEREO_MODES[plain_for::<60>(values[stereo]) as usize],
        feedback: values[feedback],
        mix: values[mix],
    }
}

pub fn reverb(values: &[f64; COUNT], slot: usize) -> crate::reverb::ReverbParameters {
    let [_, _, _, _, _, _, _, _, decay, damping, size, mix, ..] = EFFECT_SLOTS[slot];
    crate::reverb::ReverbParameters {
        decay_seconds: plain_for::<63>(values[decay]),
        damping_hz: plain_for::<64>(values[damping]),
        size: plain_for::<65>(values[size]) / 100.0,
        mix: values[mix],
    }
}

pub fn chorus(values: &[f64; COUNT], slot: usize) -> crate::chorus::ChorusParameters {
    let [time, depth, rate, detune, feedback, mix] =
        [12, 13, 14, 15, 16, 17].map(|i| EFFECT_SLOTS[slot][i]);
    crate::chorus::ChorusParameters {
        time_ms: plain_for::<75>(values[time]),
        depth_ms: plain_for::<76>(values[depth]),
        rate_hz: plain_for::<77>(values[rate]),
        detune: plain_for::<78>(values[detune]),
        feedback: plain_for::<79>(values[feedback]) / 100.0,
        mix: values[mix],
    }
}

pub fn phaser(values: &[f64; COUNT], slot: usize) -> crate::phaser::PhaserParameters {
    let [center, depth, rate, feedback, mix] = [18, 19, 20, 21, 22].map(|i| EFFECT_SLOTS[slot][i]);
    crate::phaser::PhaserParameters {
        center_hz: plain_for::<93>(values[center]),
        depth_cents: plain_for::<94>(values[depth]),
        rate_hz: plain_for::<95>(values[rate]),
        feedback: plain_for::<96>(values[feedback]) / 100.0,
        mix: values[mix],
    }
}

pub fn compressor(values: &[f64; COUNT], slot: usize) -> crate::compressor::CompressorParameters {
    let [.., threshold, ratio, attack, release, knee, makeup, mix] = EFFECT_SLOTS[slot];
    crate::compressor::CompressorParameters {
        threshold_db: plain_for::<109>(values[threshold]),
        ratio: plain_for::<110>(values[ratio]),
        attack_ms: plain_for::<111>(values[attack]),
        release_ms: plain_for::<112>(values[release]),
        knee_db: plain_for::<113>(values[knee]),
        makeup_db: plain_for::<114>(values[makeup]),
        mix: values[mix],
    }
}

pub fn envelope(values: &[f64; COUNT], first: usize) -> EnvelopeParameters {
    EnvelopeParameters {
        attack_ms: to_plain(first, values[first]),
        hold_ms: to_plain(first + 1, values[first + 1]),
        decay_ms: to_plain(first + 2, values[first + 2]),
        sustain: values[first + 3],
        release_ms: to_plain(first + 4, values[first + 4]),
    }
}

/// UI/音声スレッド間で受け渡す、検証済みの正規化値。オーディオ側でロックしない。
pub struct SharedParameters {
    /// 要素順はIDSと同じ。値ごとの更新はatomicで、表示更新はDSPへ再通知しない。
    values: [AtomicU64; COUNT],
}
impl SharedParameters {
    pub fn new() -> Self {
        let initial = defaults();
        Self {
            values: std::array::from_fn(|i| AtomicU64::new(initial[i].to_bits())),
        }
    }
    pub fn get(&self, index: usize) -> f64 {
        f64::from_bits(self.values[index].load(Ordering::Relaxed))
    }
    pub fn set(&self, index: usize, value: f64) {
        self.values[index].store(canonical(index, value).to_bits(), Ordering::Relaxed);
    }
    pub fn snapshot(&self) -> [f64; COUNT] {
        std::array::from_fn(|i| self.get(i))
    }
    pub fn store(&self, values: [f64; COUNT]) {
        for (i, value) in values.into_iter().enumerate() {
            self.set(i, value);
        }
    }
}

pub fn waveform_index(waveform: Waveform) -> u32 {
    match waveform {
        Waveform::Sine => 0,
        Waveform::Triangle => 1,
        Waveform::Square => 2,
        Waveform::Sawtooth => 3,
        Waveform::Noise => 4,
    }
}

pub fn normalized(waveform: Waveform) -> f64 {
    f64::from(waveform_index(waveform)) / 4.0
}

pub fn from_normalized(value: f64) -> Option<Waveform> {
    // 5つの等幅区間へ量子化する。上端1だけは最後の値に含める。
    (0.0..=1.0)
        .contains(&value)
        .then(|| WAVEFORMS[((value * 5.0) as usize).min(4)])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effect_fields_use_the_same_conversion_in_every_slot() {
        for indices in &EFFECT_SLOTS[1..] {
            for (&first, &other) in EFFECT_SLOTS[0].iter().zip(indices) {
                assert_eq!(control_kind(first), control_kind(other));
                for step in 0..=100 {
                    let value = f64::from(step) / 100.0;
                    assert_eq!(to_plain(first, value), to_plain(other, value));
                }
            }
        }
    }

    #[test]
    fn change_mask_preserves_float_comparison_and_range_membership() {
        let mut values = [0.0; COUNT];
        let mut previous = values;
        for index in 0..COUNT {
            for (a, b) in [
                (0.0, -0.0),
                (0.0, 0.5),
                (0.5, 0.5),
                (f64::NAN, f64::NAN),
                (f64::INFINITY, f64::INFINITY),
                (-0.5, 0.5),
            ] {
                values[index] = a;
                previous[index] = b;
                let changes = ParameterChanges::new(&values, &previous);
                let mut candidates = [0; COUNT.div_ceil(64)];
                candidates[index / 64] = 1 << (index % 64);
                let selected = ParameterChanges::from_candidates(&values, &previous, candidates);
                assert_eq!(changes.words(), selected.words());
                for position in 0..COUNT {
                    assert_eq!(changes.contains(position), index == position && a != b);
                }
                if cfg!(miri) {
                    // Miriでは全SIMD laneの読出しを上で検査し、全範囲の総当たりは通常テストで行う。
                    assert_eq!(changes.any(0, COUNT), a != b);
                    continue;
                }
                for first in 0..COUNT {
                    for end in first + 1..=COUNT {
                        assert_eq!(
                            changes.any(first, end),
                            (first..end).contains(&index) && a != b
                        );
                    }
                }
            }
            values[index] = 0.0;
            previous[index] = 0.0;
        }
    }

    #[test]
    fn knob_values_increase_and_round_trip_with_exact_defaults() {
        for (index, value) in defaults().into_iter().enumerate() {
            if !choices(index).is_empty() {
                continue;
            }
            let plain = to_plain(index, value);
            let displayed = tick_to_plain(index, to_tick(index, value));
            assert!(
                plain == displayed || (plain - displayed).abs() < plain.abs().max(1.0) * 1e-12,
                "default {index}: {plain} -> {displayed}"
            );
            let mut previous = f64::NEG_INFINITY;
            for tick in 0..=max_tick(index) {
                let plain = tick_to_plain(index, tick);
                assert!(tick == 0 || plain > previous, "order {index}: {tick}");
                assert_eq!(
                    to_tick(index, to_normalized(index, plain)),
                    tick,
                    "round trip {index}"
                );
                previous = plain;
            }
        }
    }

    #[test]
    fn logarithmic_knobs_select_meaningful_values_and_round_host_updates() {
        for (index, values) in [
            (
                35,
                &[
                    20.0, 50.0, 100.0, 200.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 5000.0,
                    10000.0, 20000.0,
                ][..],
            ),
            (39, &[0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0][..]),
            (63, &[0.1, 0.2, 0.5, 1.0, 2.0, 5.0, 10.0][..]),
            (
                64,
                &[500.0, 1000.0, 2000.0, 5000.0, 6000.0, 10000.0, 20000.0][..],
            ),
            (93, &[100.0, 200.0, 500.0, 1000.0, 2000.0, 4000.0][..]),
        ] {
            for &plain in values {
                let tick = to_tick(index, to_normalized(index, plain));
                assert_eq!(tick_to_plain(index, tick), plain);
            }
            // ホストから届く連続値は対数軸上で最も近い整数tickへ表示する。
            for tick in 0..max_tick(index) {
                let low = tick_to_plain(index, tick);
                let high = tick_to_plain(index, tick + 1);
                for (fraction, expected) in [(0.49, tick), (0.51, tick + 1)] {
                    let plain = low * (high / low).powf(fraction);
                    assert_eq!(to_tick(index, to_normalized(index, plain)), expected);
                }
            }
        }
    }

    #[test]
    fn note_controls_use_envelope_time_axis_and_ten_cent_ticks() {
        for tick in 0..=401 {
            let time = tick_to_plain(PORTAMENTO, tick);
            assert_eq!(time, tick_to_plain(1, tick));
            assert_eq!(to_tick(PORTAMENTO, to_normalized(PORTAMENTO, time)), tick);
        }
        for tick in 0..=120 {
            let cents = tick_to_plain(PITCH, tick);
            assert!((cents - f64::from(tick) * 10.0).abs() < 1e-10);
            assert_eq!(to_tick(PITCH, to_normalized(PITCH, cents)), tick);
        }
        assert!(!valid_plain(PITCH, -1.0));
        assert!(!valid_plain(PITCH, 1201.0));
        assert!(!valid_plain(PORTAMENTO, 10001.0));
        assert_eq!(defaults()[PORTAMENTO], 0.0);
        assert_eq!(to_plain(PITCH, defaults()[PITCH]), 200.0);
        assert_eq!(to_plain(PITCH, sine_preset()[PITCH]), 200.0);
    }

    #[test]
    fn controls_cover_all_steps_and_defaults() {
        let values = defaults();
        assert_eq!(to_plain(9, values[9]), 1.0);
        assert_eq!(to_plain(10, values[10]), 4800.0);
        assert_eq!(to_plain(11, values[11]), 1.0);
        assert_eq!(to_plain(13, values[13]), 0.0);
        assert_eq!(to_plain(19, values[19]), 0.0);
        assert_eq!(to_plain(130, values[130]), 100.0);
        assert_eq!(to_plain(131, values[131]), 0.0);
        // 対数換算した正規化値は、演算の丸め誤差を許容して比較する。
        for i in 0..5 {
            assert!((values[14 + i] - values[1 + i]).abs() < 1e-12);
        }
        for (index, max) in [
            (9, 6),
            (10, 1200),
            (11, 950),
            (13, 720),
            (22, 720),
            (25, 720),
            (28, 720),
            (24, 480),
            (27, 480),
            (19, 960),
            (20, 100),
            (21, 200),
            (29, 200),
            (30, 200),
            (31, 1),
            (32, 480),
            (33, 100),
        ] {
            for tick in 0..=max {
                let plain = tick_to_plain(index, tick);
                let normalized = to_normalized(index, plain);
                assert!(valid_plain(index, plain));
                assert!(
                    to_plain(index, normalized) == plain
                        || (to_plain(index, normalized) - plain).abs() < 2e-12
                );
                assert_eq!((normalized * f64::from(max)).round() as u32, tick);
                if choices(index).is_empty() {
                    assert_eq!(max_tick(index), max);
                    assert_eq!(to_tick(index, normalized), tick);
                }
            }
        }
    }

    #[test]
    fn delay_triplet_and_dotted_names_match_their_durations() {
        for (name, samples_at_120_bpm) in [
            ("1/16 T", 4000.0),
            ("1/16 D", 9000.0),
            ("1/2 T", 32000.0),
            ("1/2 D", 72000.0),
        ] {
            let index = DELAY_BEATS.iter().position(|&label| label == name).unwrap();
            for (slot, indices) in EFFECT_SLOTS.iter().enumerate() {
                let mut values = defaults();
                let time = indices[4];
                values[time] = to_normalized(time, index as f64);
                let p = delay(&values, slot);
                assert_eq!(
                    crate::delay::DIVISIONS[p.division] * 24000.0,
                    samples_at_120_bpm
                );
            }
        }
    }

    #[test]
    fn effect_ticks_round_trip_and_preset_bypasses() {
        assert_eq!(sine_preset()[34], 0.0);
        assert_eq!(defaults()[34], 0.0);
        for index in 34..COUNT {
            for tick in 0..=max_tick(index) {
                let plain = tick_to_plain(index, tick);
                assert!(valid_plain(index, plain), "{index}: {plain}");
                let value = to_normalized(index, plain);
                assert_eq!(to_tick(index, value), tick);
                assert!((to_plain(index, value) - plain).abs() < 1e-8);
            }
        }
    }

    #[test]
    fn volume_minimum_is_silence_and_next_tick_is_minus_59_9_db() {
        for index in [13, 22, 25, 28] {
            assert_eq!(to_normalized(index, -60.0), 0.0);
            assert_eq!(to_plain(index, 0.0), f64::NEG_INFINITY);
            assert_eq!(tick_to_plain(index, 0), f64::NEG_INFINITY);
            assert_eq!(tick_to_plain(index, 1), -59.9);
            assert_eq!(tick_to_plain(index, 600), 0.0);
            assert_eq!(tick_to_plain(index, 720), 12.0);
            assert_eq!(10.0_f64.powf(to_plain(index, 0.0) / 20.0), 0.0);
        }
    }

    #[test]
    fn waveform_values_round_trip_and_reject_invalid_values() {
        for (index, waveform) in WAVEFORMS.into_iter().enumerate() {
            assert_eq!(waveform_index(waveform) as usize, index);
            assert_eq!(from_normalized(normalized(waveform)), Some(waveform));
        }
        for (value, expected) in [
            (0.199, Waveform::Sine),
            (0.2, Waveform::Triangle),
            (0.4, Waveform::Square),
            (0.6, Waveform::Sawtooth),
            (0.8, Waveform::Noise),
            (1.0, Waveform::Noise),
        ] {
            assert_eq!(from_normalized(value), Some(expected));
        }
        for value in [-1.0, 1.1, f64::NAN, f64::INFINITY] {
            assert_eq!(from_normalized(value), None);
        }
    }

    #[test]
    fn unison_and_detune_integer_controls_round_trip() {
        for tick in 0..=6 {
            let plain = tick_to_plain(6, tick);
            let normalized = to_normalized(6, plain);
            assert_eq!(plain, f64::from(tick + 1));
            assert_eq!(to_plain(6, normalized), plain);
            assert!((canonical(6, normalized) - normalized).abs() < 1e-12);
        }
        for tick in 0..=100 {
            let plain = tick_to_plain(7, tick);
            assert!((to_plain(7, to_normalized(7, plain)) - f64::from(tick)).abs() < 1e-12);
        }
    }

    #[test]
    fn polyphony_integer_control_round_trips_all_values() {
        for tick in 0..=31 {
            let plain = tick_to_plain(8, tick);
            let normalized = to_normalized(8, plain);
            assert_eq!(plain, f64::from(tick + 1));
            assert_eq!(to_plain(8, normalized), plain);
            assert!((canonical(8, normalized) - normalized).abs() < 1e-12);
        }
    }

    #[test]
    fn time_axes_have_zero_endpoint_and_one_decade_per_hundred_ticks() {
        for i in [1, 2, 3, 5, 14, 15, 16, 18] {
            for (tick, ms) in [
                (0.0, 0.0),
                (1.0, 1.0),
                (101.0, 10.0),
                (201.0, 100.0),
                (301.0, 1000.0),
            ] {
                assert!((to_plain(i, tick / f64::from(TIME_TICKS)) - ms).abs() < 1e-10);
                assert!((to_normalized(i, ms) * f64::from(TIME_TICKS) - tick).abs() < 1e-10);
            }
            assert_eq!(to_plain(i, 1.0), 10000.0);
            // GUIで選べる全tickの往復変換・単調性・100tickごとの倍率を確認。
            let mut previous = -1.0;
            for tick in 0..=TIME_TICKS {
                let normalized = f64::from(tick) / f64::from(TIME_TICKS);
                let ms = tick_to_plain(i, tick);
                assert_eq!(to_tick(i, to_normalized(i, ms)), tick);
                assert!(ms > previous && ms <= 10000.0);
                assert!((to_normalized(i, ms) - normalized).abs() < 1e-12);
                assert!(ms == 0.0 || (to_plain(i, to_normalized(i, ms)) / ms - 1.0).abs() < 1e-12);
                if tick > 100 {
                    let earlier = to_plain(i, f64::from(tick - 100) / f64::from(TIME_TICKS));
                    assert!((ms / earlier - 10.0).abs() < 1e-12);
                }
                previous = ms;
            }
            assert_eq!(tick_to_plain(i, 0), 0.0);
            for (tick, ms) in [(1, 1.0), (101, 10.0), (201, 100.0), (301, 1000.0)] {
                assert!((tick_to_plain(i, tick) / ms - 1.0).abs() < 1e-12);
            }
            assert_eq!(tick_to_plain(i, 401), 10000.0);
            assert_eq!(max_tick(i), 401);
            assert_eq!(to_tick(i, 1.0), 401);
            // ホストの連続オートメーションは0〜1ms区間も線形で扱う。
            assert!((to_plain(i, to_normalized(i, 0.5)) - 0.5).abs() < 1e-12);
        }
        assert_eq!(to_plain(4, 0.35), 0.35);
        assert_eq!(to_normalized(4, 0.35), 0.35);
        for tick in 0..=100 {
            let value = tick_to_plain(4, tick);
            assert_eq!((to_normalized(4, value) * 100.0).round() as u32, tick);
        }
    }
}

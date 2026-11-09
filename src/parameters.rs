// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 音源パラメーターの値変換、VST3変更キューと保存ストリームの読み書き。

use crate::abi::*;
use crate::distortion::{ClipperType, DistortionParameters};
use crate::envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use crate::filter::FilterType;
use crate::oscillator::{PhaseMode, Waveform};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

pub const COUNT: usize = 60;
/// 各スロットの種類、Clipper/Gain/Mix、Delayの音価/Feedback/Mix。IDは位置に固定する。
pub const EFFECT_SLOTS: [[usize; 7]; 3] = [
    [42, 31, 32, 33, 51, 52, 53],
    [43, 45, 46, 47, 54, 55, 56],
    [44, 48, 49, 50, 57, 58, 59],
];
pub const EFFECT_TYPES: [crate::effects::EffectType; 3] = [
    crate::effects::EffectType::Off,
    crate::effects::EffectType::Distortion,
    crate::effects::EffectType::Delay,
];
pub const DELAY_BEATS: [&str; 10] = [
    "1/32", "1/16", "1/8 T", "1/8", "1/4 T", "1/8 D", "1/4", "1/4 D", "1/2", "1/1",
];

/// 同じ種類の設定は全スロットで同じ単位・量子化を用いる。
pub fn control_kind(index: usize) -> usize {
    match index {
        45 | 48 => 31,
        46 | 49 => 32,
        47 | 50 => 33,
        42..=44 => 31,
        54 | 57 => 51,
        55 | 58 => 52,
        53 | 56 | 59 => 33,
        _ => index,
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

pub const CLIPPER_TYPES: [ClipperType; 3] =
    [ClipperType::None, ClipperType::Soft, ClipperType::Hard];
pub const CLIPPER_NAMES: [&str; 3] = ["Off", "Soft", "Hard"];

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

/// 波形IDは従来どおり。追加IDも保存済みオートメーションとの対応を維持する。
pub const IDS: [u32; COUNT] = [
    WAVEFORM_ID,
    0x4154544b,
    0x484f4c44,
    0x44454359,
    0x53555354,
    0x52454c53,
    0x554e4953, // UNIS
    0x4454554e, // DTUN
    0x504f4c59, // POLY
    0x46545950, // FTYP
    0x46435554, // FCUT
    0x46524553, // FRES
    0x50484153, // PHAS
    0x4d564f4c, // MVOL
    0x43415454, // CATT
    0x43484c44, // CHLD
    0x43444543, // CDEC
    0x43535553, // CSUS
    0x4352454c, // CREL
    0x43414d54, // CAMT
    0x57494454, // WIDT
    0x4d50414e, // MPAN
    0x4f31564c, // O1VL
    0x4f325746, // O2WF
    0x4f32544e, // O2TN
    0x4f32564c, // O2VL
    0x4f335746, // O3WF
    0x4f33544e, // O3TN
    0x4f33564c, // O3VL
    0x4f32464d, // O2FM
    0x4f33464d, // O3FM
    0x43545950, // CTYP
    0x43474149, // CGAI
    0x434d4958, // CMIX
    0x4551_0000,
    0x4551_0001,
    0x4551_0002,
    0x4551_0003,
    0x4551_0004,
    0x4551_0005,
    0x4551_0006,
    0x4551_0007,
    0x4546_3154, // EF1T
    0x4546_3254, // EF2T
    0x4546_3354, // EF3T
    0x4546_3243, // EF2C
    0x4546_3247, // EF2G
    0x4546_324d, // EF2M
    0x4546_3343, // EF3C
    0x4546_3347, // EF3G
    0x4546_334d, // EF3M
    0x4546_3142,
    0x4546_3146,
    0x4546_3144, // EF1B / EF1F / EF1D
    0x4546_3242,
    0x4546_3246,
    0x4546_3244,
    0x4546_3342,
    0x4546_3346,
    0x4546_3344,
];
pub const NAMES: [&str; COUNT] = [
    "Osc1 Waveform",
    "Attack",
    "Hold",
    "Decay",
    "Sustain",
    "Release",
    "Unison",
    "Detune",
    "Polyphony",
    "Filter",
    "Cutoff",
    "Resonance",
    "Phase",
    "Volume",
    "Cutoff Attack",
    "Cutoff Hold",
    "Cutoff Decay",
    "Cutoff Sustain",
    "Cutoff Release",
    "Cutoff Amount",
    "Width",
    "Pan",
    "Osc1 Volume",
    "Osc2 Waveform",
    "Osc2 Tune",
    "Osc2 Volume",
    "Osc3 Waveform",
    "Osc3 Tune",
    "Osc3 Volume",
    "Osc2 Mod",
    "Osc3 Mod",
    "Effect 1 Clipper",
    "Effect 1 Gain",
    "Effect 1 Mix",
    "EQ Enable",
    "EQ Low Frequency",
    "EQ Low Gain",
    "EQ Mid Frequency",
    "EQ Mid Gain",
    "EQ Mid Q",
    "EQ High Frequency",
    "EQ High Gain",
    "Effect 1",
    "Effect 2",
    "Effect 3",
    "Effect 2 Clipper",
    "Effect 2 Gain",
    "Effect 2 Mix",
    "Effect 3 Clipper",
    "Effect 3 Gain",
    "Effect 3 Mix",
    "Effect 1 Beat",
    "Effect 1 Feedback",
    "Effect 1 Delay Mix",
    "Effect 2 Beat",
    "Effect 2 Feedback",
    "Effect 2 Delay Mix",
    "Effect 3 Beat",
    "Effect 3 Feedback",
    "Effect 3 Delay Mix",
];
/// 離散リストの表示順。連続値や数値ノブでは空のスライスを返す。
pub fn choices(index: usize) -> &'static [&'static str] {
    if (42..=44).contains(&index) {
        return &["Off", "Distortion", "Delay"];
    }
    match control_kind(index) {
        0 | 23 | 26 => &WAVEFORM_NAMES,
        9 => &FILTER_NAMES,
        12 => &PHASE_NAMES,
        31 => &CLIPPER_NAMES,
        34 => &["Off", "On"],
        51 => &DELAY_BEATS,
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
            34 => 1,
            31 => 2,
            51 => 9,
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
            _ => 100,
        }
    }
}

/// ホストの正規化値をGUIの整数tickへ換算する。表示更新だけでは再通知しない。
pub fn to_tick(index: usize, value: f64) -> u32 {
    (value * f64::from(max_tick(index))).round() as u32
}

/// GUIとホストへ公開する実単位。
pub fn unit(index: usize) -> &'static str {
    let index = control_kind(index);
    if is_time(index) {
        "ms"
    } else {
        match index {
            35 | 37 | 40 => "Hz",
            36 | 38 | 41 => "dB",
            13 | 22 | 25 | 28 | 32 => "dB",
            20 | 21 | 29 | 30 | 33 | 52 => "%",
            7 | 10 | 19 | 24 | 27 => "cent",
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
    matches!(index, 1 | 2 | 3 | 5 | 14 | 15 | 16 | 18)
}
/// 変更点まで直前の値を保持するパラメーター。Cutoffは連続値だが補間しない。
pub fn holds_between_points(index: usize) -> bool {
    matches!(
        index,
        0 | 6 | 8 | 9 | 10 | 12 | 23 | 26 | 31 | 34 | 42..=45 | 48 | 51 | 54 | 57 | PROGRAM_INDEX
    )
}
/// GUIの整数tickを実単位へ換算する。時間0..=401、sustain/detune/width 0..=100、unison 0..=6、polyphony 0..=31、cutoff 0..=1200、resonance 0..=950、volume 0..=720（0は−∞）、cutoff amount 0..=960、pan 0..=200、tune 0..=480。
pub fn tick_to_plain(index: usize, tick: u32) -> f64 {
    let index = control_kind(index);
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
    let index = control_kind(index);
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
        34 => return (normalized * 2.0).floor().min(1.0),
        51 => return (normalized * 10.0).floor().min(9.0),
        52 => return normalized * 100.0,
        35 | 37 | 40 => return 20.0 * 1000_f64.powf(normalized),
        36 | 38 | 41 => return normalized * 36.0 - 18.0,
        39 => return 0.1 * 100_f64.powf(normalized),
        4 | 17 => return normalized,
        6 => return (normalized * 7.0).floor().min(6.0) + 1.0,
        7 | 20 | 33 => return normalized * 100.0,
        32 => return normalized * 48.0,
        29 | 30 => return normalized * 200.0,
        21 => return normalized * 200.0 - 100.0,
        8 => return (normalized * 32.0).floor().min(31.0) + 1.0,
        9 => return (normalized * 7.0).floor().min(6.0),
        10 => return normalized * 12000.0 - 2400.0,
        11 => return 0.5 + normalized * 9.5,
        12 | 31 => return (normalized * 3.0).floor().min(2.0),
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
        34 => return plain.clamp(0.0, 1.0).round(),
        51 => return plain.clamp(0.0, 9.0).round() / 9.0,
        52 => return plain / 100.0,
        35 | 37 | 40 => return (plain / 20.0).log(1000.0),
        36 | 38 | 41 => return (plain + 18.0) / 36.0,
        39 => return (plain / 0.1).log(100.0),
        4 | 17 => return plain,
        6 => return (plain.clamp(1.0, 7.0).round() - 1.0) / 6.0,
        7 | 20 | 33 => return plain / 100.0,
        32 => return plain / 48.0,
        29 | 30 => return plain / 200.0,
        21 => return (plain + 100.0) / 200.0,
        8 => return (plain.clamp(1.0, 32.0).round() - 1.0) / 31.0,
        9 => return plain.clamp(0.0, 6.0).round() / 6.0,
        10 => return (plain + 2400.0) / 12000.0,
        11 => return (plain - 0.5) / 9.5,
        12 | 31 => return plain.clamp(0.0, 2.0).round() / 2.0,
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
        34 => (0.0..=1.0).contains(&plain) && plain.fract() == 0.0,
        51 => (0.0..=9.0).contains(&plain) && plain.fract() == 0.0,
        52 => (0.0..=100.0).contains(&plain),
        35 | 37 | 40 => (20.0..=20000.0).contains(&plain),
        36 | 38 | 41 => (-18.0..=18.0).contains(&plain),
        39 => (0.1..=10.0).contains(&plain),
        4 | 17 => (0.0..=1.0).contains(&plain),
        6 => (1.0..=7.0).contains(&plain) && plain.fract() == 0.0,
        7 | 20 | 33 => (0.0..=100.0).contains(&plain),
        29 | 30 => (0.0..=200.0).contains(&plain),
        21 => (-100.0..=100.0).contains(&plain),
        8 => (1.0..=32.0).contains(&plain) && plain.fract() == 0.0,
        9 => (0.0..=6.0).contains(&plain) && plain.fract() == 0.0,
        10 => (-2400.0..=9600.0).contains(&plain),
        11 => (0.5..=10.0).contains(&plain),
        12 | 31 => (0.0..=2.0).contains(&plain) && plain.fract() == 0.0,
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
    } else if matches!(control_kind(index), 6 | 8 | 9 | 12 | 31 | 34 | 51) {
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
    let plain: [f64; COUNT] = [
        0.0,
        p.attack_ms,
        p.hold_ms,
        p.decay_ms,
        p.sustain,
        p.release_ms,
        1.0,
        0.0,
        1.0,
        f64::from(filter as u32),
        4800.0, // ノートから+4 oct
        1.0,
        0.0, // Phase: Keep
        0.0, // Volume: 0 dB
        p.attack_ms,
        p.hold_ms,
        p.decay_ms,
        p.sustain,
        p.release_ms,
        0.0, // Cutoff Amount: 0 cent
        0.0, // Width: 0%
        0.0, // Pan: 中央
        0.0, // Osc1 Volume
        0.0,
        0.0,
        f64::NEG_INFINITY, // Osc2
        0.0,
        0.0,
        f64::NEG_INFINITY, // Osc3
        0.0,               // Osc2 Mod
        0.0,               // Osc3 Mod
        1.0,               // Clipper: Soft（スロット自体はOff）
        0.0,               // Gain: 0 dB
        100.0,             // Mix: 100%
        0.0,
        250.0,
        0.0,
        1000.0,
        0.0,
        1.0,
        4000.0,
        0.0,
        0.0,
        0.0,
        0.0, // 3スロットともOff
        1.0,
        0.0,
        100.0, // Effect 2: Soft / 0 dB / 100%
        1.0,
        0.0,
        100.0, // Effect 3
        3.0,
        35.0,
        25.0,
        3.0,
        35.0,
        25.0,
        3.0,
        35.0,
        25.0, // Delay: 1/8, 35%, 25%
    ];
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
        clipper: CLIPPER_TYPES[to_plain(clipper, values[clipper]) as usize],
        gain: 10.0_f64.powf(to_plain(gain, values[gain]) / 20.0),
        mix: values[mix],
    }
}

pub fn delay(values: &[f64; COUNT], slot: usize) -> crate::delay::DelayParameters {
    let [_, _, _, _, beat, feedback, mix] = EFFECT_SLOTS[slot];
    crate::delay::DelayParameters {
        division: to_plain(beat, values[beat]) as usize,
        feedback: values[feedback],
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

/// 1回のprocess中だけホストから借用する、1パラメーターの変更点のカーソル。
pub struct ParameterPoints<'a> {
    /// ホスト所有のIParamValueQueue。利用可能なキューがなければnull。
    queue: This,
    /// dataと同じ呼び出し中だけ有効な関数表。所有参照を取得しない。
    table: Option<&'a ParamValueQueueVTable>,
    /// キューが申告した変更点数。
    count: i32,
    /// 次に読み出す変更点の列挙位置。
    index: i32,
    /// 最後に受理したoffset。後戻りする変更点を除外するために保持する。
    previous_offset: i32,
    /// 今回のブロック長。0ではoffset 0だけを受理する。
    samples: i32,
    /// 次に適用する変更点。同じoffsetの変更はキュー順に適用する。
    next: Option<(i32, f64)>,
    /// 補間の始点。前ブロックの最終値をoffset=-1として扱う。
    previous: (i32, f64),
    /// holds_between_pointsの指定値は変更点まで保持し、それ以外は正規化空間で線形補間する。
    parameter: usize,
    /// キュー作成時に確定する補間モード。サンプルごとのParamID分類を省く。
    interpolates: bool,
    /// 有効な変更点があるキューだけをDSPから共有値へ書き戻す。
    /// 他パラメーターの状態復元を、無関係なオートメーションで上書きしない。
    pub automated: bool,
}

impl<'a> ParameterPoints<'a> {
    /// ホストのキューを1回列挙し、ParamIDごとのカーソルを構成する。
    /// SAFETY: dataの入れ子のインターフェースはホスト所有で、dataの借用期間中有効。
    pub unsafe fn all(data: &'a ProcessData, values: &[f64; COUNT]) -> [Self; COUNT + 1] {
        let mut points = std::array::from_fn(|i| {
            Self::empty(data.num_samples, i, values.get(i).copied().unwrap_or(0.0))
        });
        if !data.input_parameter_changes.is_null() {
            // SAFETY: ホストのIParameterChangesと借用キューを読み、process中だけ保持する。
            unsafe {
                let changes = data.input_parameter_changes;
                let table = &**changes.cast::<*const ParameterChangesVTable>();
                for at in 0..(table.get_parameter_count)(changes) {
                    let queue = (table.get_parameter_data)(changes, at);
                    if queue.is_null() {
                        continue;
                    }
                    let queue_table = &**queue.cast::<*const ParamValueQueueVTable>();
                    let id = (queue_table.get_parameter_id)(queue);
                    let parameter = if id == PROGRAM_ID {
                        Some(PROGRAM_INDEX)
                    } else {
                        index(id)
                    };
                    let Some(parameter) = parameter else {
                        continue;
                    };
                    let point = &mut points[parameter];
                    if point.table.is_some() {
                        continue; // ParamID重複時は最初のキューを採用する。
                    }
                    point.queue = queue;
                    point.table = Some(queue_table);
                    point.count = (queue_table.get_point_count)(queue);
                    point.advance();
                    point.automated = point.next.is_some();
                }
            }
        }
        points
    }

    fn empty(samples: i32, parameter: usize, previous_value: f64) -> Self {
        Self {
            queue: ptr::null_mut(),
            table: None,
            count: 0,
            index: 0,
            previous_offset: 0,
            samples,
            next: None,
            previous: (-1, previous_value),
            parameter,
            interpolates: !holds_between_points(parameter),
            automated: false,
        }
    }

    /// Programは同じ番号の再選択もロード要求として扱う。
    pub fn take_program_at(&mut self, offset: i32) -> bool {
        let changed = self.next.is_some_and(|(at, _)| at <= offset);
        self.value_at(offset);
        changed
    }

    /// プログラム切替点を次の補間の始点とする。同位置の明示的な変更点を優先する。
    pub fn reset_value(&mut self, offset: i32, value: f64) {
        self.previous = (offset, value);
    }

    pub fn value_at(&mut self, offset: i32) -> f64 {
        while let Some((at, value)) = self.next {
            if at > offset {
                break;
            }
            self.previous = (at, value);
            self.advance();
        }
        if self.interpolates
            && let Some((at, value)) = self.next
        {
            let fraction = f64::from(offset - self.previous.0) / f64::from(at - self.previous.0);
            return self.previous.1 + (value - self.previous.1) * fraction;
        }
        self.previous.1
    }

    pub fn advance(&mut self) {
        self.next = None;
        let Some(table) = self.table else {
            return;
        };
        while self.index < self.count {
            let mut offset = -1;
            let mut value = f64::NAN;
            // SAFETY: allの契約によりキューは生存中。出力先は型の一致したローカル変数。
            let status =
                unsafe { (table.get_point)(self.queue, self.index, &mut offset, &mut value) };
            self.index += 1;
            if status != OK
                || offset < self.previous_offset
                || (self.samples == 0 && offset != 0)
                || (self.samples > 0 && offset >= self.samples)
            {
                continue;
            }
            if (0.0..=1.0).contains(&value) {
                self.previous_offset = offset;
                self.next = Some((offset, canonical(self.parameter, value)));
                break;
            }
        }
    }
}

// v25: 固定484バイト。波形番号u32と、その他の実単位f64を保持する。
const STATE_SIZE: usize = 12 + (COUNT - 1) * 8;
const STATE_HEADER: [u8; 8] = *b"SVST\x19\0\0\0";

/// SAFETY: streamは呼び出し中有効なホスト所有のIBStream、またはnull。
pub unsafe fn read_state(stream: This) -> Result<[f64; COUNT], TResult> {
    if stream.is_null() {
        return Err(INVALID_ARGUMENT);
    }
    // SAFETY: IBStreamの関数表を借用し、固定長領域の範囲だけを読み取る。
    let table = unsafe { &**stream.cast::<*const StreamVTable>() };
    let mut bytes = [0u8; STATE_SIZE];
    let mut position = 0;
    while position < bytes.len() {
        let mut read = 0;
        let remaining = bytes.len() - position;
        let status = unsafe {
            (table.read)(
                stream,
                bytes.as_mut_ptr().add(position).cast(),
                remaining as i32,
                &mut read,
            )
        };
        if status != OK || read <= 0 || read as usize > remaining {
            return Err(FALSE);
        }
        position += read as usize;
    }
    if bytes[..8] != STATE_HEADER {
        return Err(FALSE);
    }
    let waveform = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if waveform > 4 {
        return Err(FALSE);
    }
    let mut values = [0.0; COUNT];
    values[0] = f64::from(waveform) / 4.0;
    for (i, value) in values.iter_mut().enumerate().skip(1) {
        let offset = 12 + (i - 1) * 8;
        let plain = f64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
        if !valid_plain(i, plain) {
            return Err(FALSE);
        }
        *value = to_normalized(i, plain);
    }
    Ok(values)
}

/// SAFETY: streamは呼び出し中有効なホスト所有のIBStream、またはnull。
pub unsafe fn write_state(stream: This, values: [f64; COUNT]) -> TResult {
    if stream.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: IBStreamは固定長バッファを呼び出し中だけ読む。
    let table = unsafe { &**stream.cast::<*const StreamVTable>() };
    let mut bytes = [0u8; STATE_SIZE];
    bytes[..8].copy_from_slice(&STATE_HEADER);
    bytes[8..12].copy_from_slice(&(to_plain(0, values[0]) as u32).to_le_bytes());
    for (i, &value) in values.iter().enumerate().skip(1) {
        let offset = 12 + (i - 1) * 8;
        bytes[offset..offset + 8].copy_from_slice(&to_plain(i, value).to_le_bytes());
    }
    let mut position = 0;
    while position < bytes.len() {
        let mut written = 0;
        let remaining = bytes.len() - position;
        let status = unsafe {
            (table.write)(
                stream,
                bytes.as_mut_ptr().add(position).cast(),
                remaining as i32,
                &mut written,
            )
        };
        if status != OK || written <= 0 || written as usize > remaining {
            return FALSE;
        }
        position += written as usize;
    }
    OK
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sample_positions_interpolate_only_known_continuous_intervals() {
        let mut points = ParameterPoints::empty(8, 32, 0.2);
        points.previous = (0, 0.2);
        points.next = Some((4, 1.0));
        for (at, expected) in [(0, 0.2), (1, 0.4), (2, 0.6), (3, 0.8)] {
            assert!((points.value_at(at) - expected).abs() < 1e-15);
        }
        points.next = None;
        assert_eq!(points.value_at(1), 0.2);
        points.next = Some((4, 1.0));
        points.interpolates = false;
        assert_eq!(points.value_at(1), 0.2);
    }

    #[repr(C)]
    struct Stream {
        /// このテストだけで使うIBStream関数表。
        vtable: &'static StreamVTable,
        /// 形式の最大長だけを用意したテスト用領域。
        bytes: [u8; STATE_SIZE],
        /// 初期化済みデータの長さ。
        length: usize,
        /// 次の読み書き位置。
        position: usize,
    }

    unsafe extern "system" fn query(_: This, _: *const Tuid, _: *mut This) -> TResult {
        NO_INTERFACE
    }
    unsafe extern "system" fn retain(_: This) -> u32 {
        panic!("stream is borrowed")
    }
    unsafe extern "system" fn read(this: This, out: This, size: i32, actual: *mut i32) -> TResult {
        // SAFETY: テストのStreamと、呼び出し元が確保した出力領域。両者は重ならない。
        unsafe {
            let stream = &mut *this.cast::<Stream>();
            let count = (size as usize).min(3).min(stream.length - stream.position);
            ptr::copy_nonoverlapping(
                stream.bytes.as_ptr().add(stream.position),
                out.cast(),
                count,
            );
            stream.position += count;
            actual.write(count as i32);
        }
        OK
    }
    unsafe extern "system" fn write(
        this: This,
        input: This,
        size: i32,
        actual: *mut i32,
    ) -> TResult {
        // SAFETY: テストのStreamと、呼び出し元が確保した入力領域。両者は重ならない。
        unsafe {
            let stream = &mut *this.cast::<Stream>();
            let count = (size as usize)
                .min(2)
                .min(stream.bytes.len() - stream.position);
            ptr::copy_nonoverlapping(
                input.cast(),
                stream.bytes.as_mut_ptr().add(stream.position),
                count,
            );
            stream.position += count;
            stream.length = stream.position;
            actual.write(count as i32);
        }
        OK
    }
    unsafe extern "system" fn seek(_: This, _: i64, _: i32, _: *mut i64) -> TResult {
        panic!("unexpected seek")
    }
    unsafe extern "system" fn tell(_: This, _: *mut i64) -> TResult {
        panic!("unexpected tell")
    }
    static STREAM_TABLE: StreamVTable = StreamVTable {
        unknown: UnknownVTable {
            query_interface: query,
            add_ref: retain,
            release: retain,
        },
        read,
        write,
        seek,
        tell,
    };

    #[test]
    fn controls_cover_all_steps_and_defaults() {
        let values = defaults();
        assert_eq!(to_plain(9, values[9]), 1.0);
        assert_eq!(to_plain(10, values[10]), 4800.0);
        assert_eq!(to_plain(11, values[11]), 1.0);
        assert_eq!(to_plain(13, values[13]), 0.0);
        assert_eq!(to_plain(19, values[19]), 0.0);
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
            (31, 2),
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
    fn state_stream_handles_short_transfers_and_rejects_truncation() {
        for waveform in WAVEFORMS {
            let mut stream = Stream {
                vtable: &STREAM_TABLE,
                bytes: [0; STATE_SIZE],
                length: 0,
                position: 0,
            };
            let stream = ptr::from_mut(&mut stream);
            // SAFETY: スタック上のStreamを同じポインターから順に使う。借用を外へ保持しない。
            unsafe {
                let mut values = defaults();
                for index in [1, 2, 3, 5] {
                    values[index] = to_normalized(index, 10000.0);
                }
                for (i, v) in values.iter_mut().enumerate().skip(34) {
                    *v = canonical(i, 0.625);
                }
                values[0] = normalized(waveform);
                values[6] = to_normalized(6, 7.0);
                values[7] = to_normalized(7, 37.125);
                values[8] = to_normalized(8, 32.0);
                values[9] = to_normalized(9, 6.0);
                values[10] = to_normalized(10, -1200.125);
                values[11] = to_normalized(11, 5.125);
                values[12] = to_normalized(12, 2.0);
                values[13] = to_normalized(13, -59.95);
                values[20] = to_normalized(20, 62.5);
                values[21] = to_normalized(21, -35.25);
                for (i, plain) in [7500.25, 10000.0, 6000.0, 0.4567, 10000.0, -1234.125]
                    .into_iter()
                    .enumerate()
                {
                    values[14 + i] = to_normalized(14 + i, plain);
                }
                assert_eq!(write_state(stream.cast(), values), OK);
                (*stream).position = 0;
                let restored = read_state(stream.cast()).unwrap();
                for (actual, expected) in restored.into_iter().zip(values) {
                    assert!((actual - expected).abs() < 1e-12);
                }
                for length in 0..STATE_SIZE {
                    (*stream).length = length;
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                }
                (*stream).length = STATE_SIZE;
                for version in (0..=32).filter(|v| *v != STATE_HEADER[4]) {
                    (*stream).bytes[4] = version;
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                }
                (*stream).bytes[4] = STATE_HEADER[4];
                // ボイス数の範囲外・端数、detuneの範囲外/非有限を拒否。
                for (index, bad) in [
                    (6, 0.0_f64),
                    (6, 8.0),
                    (6, 2.5),
                    (7, -1.0),
                    (7, 101.0),
                    (7, f64::NAN),
                    (8, 0.0),
                    (8, 33.0),
                    (8, 1.5),
                    (8, f64::INFINITY),
                    (9, 7.0),
                    (9, 1.5),
                    (10, -2410.0),
                    (10, 9601.0),
                    (10, f64::NAN),
                    (11, 0.49),
                    (11, 10.1),
                    (11, f64::NAN),
                    (12, -1.0),
                    (12, 3.0),
                    (12, 1.5),
                    (12, f64::NAN),
                    (13, -60.1),
                    (13, 12.1),
                    (13, f64::NAN),
                    (13, f64::INFINITY),
                    (14, -1.0),
                    (15, 10000.1),
                    (16, f64::NAN),
                    (17, -0.1),
                    (17, 1.1),
                    (18, f64::INFINITY),
                    (19, -4800.1),
                    (19, 4800.1),
                    (19, f64::NAN),
                    (20, -0.1),
                    (20, 100.1),
                    (20, f64::NAN),
                    (21, -100.1),
                    (21, 100.1),
                    (21, f64::INFINITY),
                    (22, -60.1),
                    (25, f64::INFINITY),
                    (28, f64::NAN),
                    (23, 5.0),
                    (26, 1.5),
                    (24, -2400.1),
                    (27, 2400.1),
                    (29, -0.1),
                    (30, 200.1),
                    (29, f64::NAN),
                    (30, f64::INFINITY),
                    (31, -1.0),
                    (31, 3.0),
                    (31, 1.5),
                    (32, -0.1),
                    (32, 48.1),
                    (32, f64::NAN),
                    (33, -0.1),
                    (33, 100.1),
                    (33, f64::INFINITY),
                    (34, -1.0),
                    (34, 2.0),
                    (34, 0.5),
                    (34, f64::NAN),
                    (35, 19.0),
                    (35, 20001.0),
                    (35, f64::INFINITY),
                    (36, -18.1),
                    (36, 18.1),
                    (36, f64::NAN),
                    (37, 19.0),
                    (37, 20001.0),
                    (37, f64::NAN),
                    (38, -18.1),
                    (38, 18.1),
                    (38, f64::INFINITY),
                    (39, 0.09),
                    (39, 10.1),
                    (39, f64::INFINITY),
                    (40, 19.0),
                    (40, 20001.0),
                    (40, f64::INFINITY),
                    (41, -18.1),
                    (41, 18.1),
                    (41, f64::NAN),
                ] {
                    let offset = 12 + (index - 1) * 8;
                    let original: [u8; 8] =
                        (&(*stream).bytes)[offset..offset + 8].try_into().unwrap();
                    (&mut (*stream).bytes)[offset..offset + 8].copy_from_slice(&bad.to_le_bytes());
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                    (&mut (*stream).bytes)[offset..offset + 8].copy_from_slice(&original);
                }
            }
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

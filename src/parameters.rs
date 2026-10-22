// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 音源パラメーターの値変換、VST3変更キューと保存ストリームの読み書き。

use crate::abi::*;
use crate::envelope::{DEFAULT_ENVELOPE, EnvelopeParameters};
use crate::filter::FilterType;
use crate::oscillator::{PhaseMode, Waveform};
use std::ptr;
use std::sync::atomic::{AtomicU64, Ordering};

pub const COUNT: usize = 20;
pub const PHASE_MODES: [PhaseMode; 3] = [PhaseMode::Keep, PhaseMode::Zero, PhaseMode::Random];
pub const PHASE_NAMES: [&str; 3] = ["Keep", "Zero", "Random"];

/// ホストのオートメーションとの対応を維持する固定ID（"WAVE"）。
pub const WAVEFORM_ID: u32 = 0x5741_5645;
/// 値・保存形式・表示順の対応。既存の番号を変更しない。
pub const WAVEFORMS: [Waveform; 4] = [
    Waveform::Sine,
    Waveform::Triangle,
    Waveform::Square,
    Waveform::Sawtooth,
];
pub const WAVEFORM_NAMES: [&str; 4] = ["Sine", "Triangle", "Square", "Sawtooth"];
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
    "None",
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
];
pub const NAMES: [&str; COUNT] = [
    "Waveform",
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
];
/// VST3正規化値と時間を換算する連続軸のスケール。UIの整数tick上限とは区別する。
pub fn time_scale() -> f64 {
    1.0 + 100.0 * 5000.0_f64.log10()
}
pub fn index(id: u32) -> Option<usize> {
    IDS.iter().position(|&candidate| candidate == id)
}
pub fn is_time(index: usize) -> bool {
    matches!(index, 1 | 2 | 3 | 5 | 14 | 15 | 16 | 18)
}
/// 変更点まで直前の値を保持するパラメーター。Cutoffは連続値だが補間しない。
pub fn holds_between_points(index: usize) -> bool {
    matches!(index, 0 | 6 | 8 | 9 | 10 | 12)
}
/// GUIの整数tickを実単位へ換算する。時間0..=371、sustain/detune 0..=100、unison 0..=6、polyphony 0..=31、cutoff 0..=1200、resonance 0..=950、volume 0..=600、cutoff amount 0..=960。
pub fn slider_to_plain(index: usize, tick: u32) -> f64 {
    if is_time(index) {
        if tick == 0 {
            0.0
        } else {
            10.0_f64.powf((f64::from(tick) - 1.0) / 100.0).min(5000.0)
        }
    } else {
        match index {
            6 | 8 => f64::from(tick) + 1.0,
            7 | 9 => f64::from(tick),
            10 => f64::from(tick) * 10.0 - 2400.0,
            11 => 0.5 + f64::from(tick) / 100.0,
            13 => f64::from(tick) / 10.0 - 48.0,
            19 => f64::from(tick) * 10.0 - 4800.0,
            _ => f64::from(tick) / 100.0,
        }
    }
}
pub fn to_plain(index: usize, normalized: f64) -> f64 {
    if index == 0 {
        return from_normalized(normalized).map_or(0.0, |w| f64::from(waveform_index(w)));
    }
    match index {
        4 | 17 => return normalized,
        6 => return (normalized * 7.0).floor().min(6.0) + 1.0,
        7 => return normalized * 100.0,
        8 => return (normalized * 32.0).floor().min(31.0) + 1.0,
        9 => return (normalized * 7.0).floor().min(6.0),
        10 => return normalized * 12000.0 - 2400.0,
        11 => return 0.5 + normalized * 9.5,
        12 => return (normalized * 3.0).floor().min(2.0),
        13 => return normalized * 60.0 - 48.0,
        19 => return normalized * 9600.0 - 4800.0,
        _ => {}
    }
    if normalized == 1.0 {
        return 5000.0;
    }
    let tick = normalized * time_scale();
    if tick <= 1.0 {
        tick
    } else {
        10.0_f64.powf((tick - 1.0) / 100.0).min(5000.0)
    }
}
pub fn to_normalized(index: usize, plain: f64) -> f64 {
    if index == 0 {
        return plain.clamp(0.0, 3.0).round() / 3.0;
    }
    match index {
        4 | 17 => return plain,
        6 => return (plain.clamp(1.0, 7.0).round() - 1.0) / 6.0,
        7 => return plain / 100.0,
        8 => return (plain.clamp(1.0, 32.0).round() - 1.0) / 31.0,
        9 => return plain.clamp(0.0, 6.0).round() / 6.0,
        10 => return (plain + 2400.0) / 12000.0,
        11 => return (plain - 0.5) / 9.5,
        12 => return plain.clamp(0.0, 2.0).round() / 2.0,
        13 => return (plain + 48.0) / 60.0,
        19 => return (plain + 4800.0) / 9600.0,
        _ => {}
    }
    if plain == 5000.0 {
        return 1.0;
    }
    (if plain <= 1.0 {
        plain
    } else {
        1.0 + 100.0 * plain.log10()
    }) / time_scale()
}
pub fn valid_plain(index: usize, plain: f64) -> bool {
    match index {
        0 => (0.0..=3.0).contains(&plain),
        4 | 17 => (0.0..=1.0).contains(&plain),
        6 => (1.0..=7.0).contains(&plain) && plain.fract() == 0.0,
        7 => (0.0..=100.0).contains(&plain),
        8 => (1.0..=32.0).contains(&plain) && plain.fract() == 0.0,
        9 => (0.0..=6.0).contains(&plain) && plain.fract() == 0.0,
        10 => (-2400.0..=9600.0).contains(&plain),
        11 => (0.5..=10.0).contains(&plain),
        12 => (0.0..=2.0).contains(&plain) && plain.fract() == 0.0,
        13 => (-48.0..=12.0).contains(&plain),
        19 => (-4800.0..=4800.0).contains(&plain),
        _ => (0.0..=5000.0).contains(&plain),
    }
}
/// 直接入力の実単位を検証し、スライダーの刻みに丸めずVST3正規化値へ換算する。
pub fn parse_numeric(index: usize, text: &str) -> Option<f64> {
    let plain = text.trim().parse::<f64>().ok()?;
    valid_plain(index, plain).then(|| to_normalized(index, plain))
}
pub fn canonical(index: usize, value: f64) -> f64 {
    if index == 0 {
        normalized(from_normalized(value).expect("validated normalized value"))
    } else if matches!(index, 6 | 8 | 9 | 12) {
        to_normalized(index, to_plain(index, value))
    } else {
        value
    }
}
pub fn defaults() -> [f64; COUNT] {
    let p = DEFAULT_ENVELOPE;
    let plain = [
        0.0,
        p.attack_ms,
        p.hold_ms,
        p.decay_ms,
        p.sustain,
        p.release_ms,
        1.0,
        0.0,
        1.0,
        1.0,    // LowPass1
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
    ];
    std::array::from_fn(|i| to_normalized(i, plain[i]))
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
    }
}

pub fn normalized(waveform: Waveform) -> f64 {
    f64::from(waveform_index(waveform)) / 3.0
}

pub fn from_normalized(value: f64) -> Option<Waveform> {
    // 4つの等幅区間へ量子化する。上端1だけは最後の値に含める。
    (0.0..=1.0)
        .contains(&value)
        .then(|| WAVEFORMS[((value * 4.0) as usize).min(3)])
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
    /// 有効な変更点があるキューだけをDSPから共有値へ書き戻す。
    /// 他パラメーターの状態復元を、無関係なオートメーションで上書きしない。
    pub automated: bool,
}

impl<'a> ParameterPoints<'a> {
    /// SAFETY: dataの入れ子のインターフェースはホスト所有で、dataの借用期間中有効。
    pub unsafe fn new(data: &'a ProcessData, parameter: usize, previous_value: f64) -> Self {
        let mut result = Self {
            queue: ptr::null_mut(),
            table: None,
            count: 0,
            index: 0,
            previous_offset: 0,
            samples: data.num_samples,
            next: None,
            previous: (-1, previous_value),
            parameter,
            automated: false,
        };
        if !data.input_parameter_changes.is_null() {
            // SAFETY: ホストのIParameterChangesと借用キューを読み、保持期間をprocess内に限定する。
            unsafe {
                let changes = data.input_parameter_changes;
                let table = &**changes.cast::<*const ParameterChangesVTable>();
                for index in 0..(table.get_parameter_count)(changes) {
                    let queue = (table.get_parameter_data)(changes, index);
                    if queue.is_null() {
                        continue;
                    }
                    let queue_table = &**queue.cast::<*const ParamValueQueueVTable>();
                    if (queue_table.get_parameter_id)(queue) == IDS[parameter] {
                        result.queue = queue;
                        result.table = Some(queue_table);
                        result.count = (queue_table.get_point_count)(queue);
                        break; // 仕様ではParamIDごとに1キュー。重複時は最初のキューを採用する。
                    }
                }
            }
        }
        result.advance();
        result.automated = result.next.is_some();
        result
    }

    pub fn value_at(&mut self, offset: i32) -> f64 {
        while let Some((at, value)) = self.next {
            if at > offset {
                break;
            }
            self.previous = (at, value);
            self.advance();
        }
        if !holds_between_points(self.parameter)
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
            // SAFETY: newの契約によりキューは生存中。出力先は型の一致したローカル変数。
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

// v5: v4の76バイトにフィルタ3値の実単位f64を追加（100バイト、little endian）。
// v6: v5にPhaseの実単位f64を追加（108バイト）。v1〜v5はPhase=Keepを補う。
// v1〜v4はフィルタNoneとして旧音色を維持する。空状態は新規設定へ戻す。
// v7: v6にVolumeのdB値f64を追加（116バイト）。v1〜v6は0 dBを補う。
// v8: v7にカットオフAHDSRとAmountの実単位f64を追加（164バイト）。旧形式のAmountは0。
const STATE_HEADER: [u8; 8] = *b"SVST\x08\0\0\0";

/// SAFETY: streamは呼び出し中有効なホスト所有のIBStream、またはnull。
pub unsafe fn read_state(stream: This) -> Result<[f64; COUNT], TResult> {
    if stream.is_null() {
        return Err(INVALID_ARGUMENT);
    }
    // SAFETY: IBStreamの関数表を借用し、固定長領域の範囲だけを読み取る。
    let table = unsafe { &**stream.cast::<*const StreamVTable>() };
    let mut bytes = [0u8; 164];
    let mut position = 0;
    let mut length = 12;
    while position < length {
        let mut read = 0;
        let remaining = length - position;
        let status = unsafe {
            (table.read)(
                stream,
                bytes.as_mut_ptr().add(position).cast(),
                remaining as i32,
                &mut read,
            )
        };
        if position == 0 && read == 0 && matches!(status, OK | FALSE) {
            return Ok(defaults());
        }
        if status != OK || read <= 0 || read as usize > remaining {
            return Err(FALSE);
        }
        position += read as usize;
        if position == 12 {
            if bytes[..4] != *b"SVST" {
                return Err(FALSE);
            }
            match u32::from_le_bytes(bytes[4..8].try_into().unwrap()) {
                1 => {}
                2 => length = 52,
                3 => length = 68,
                4 => length = 76,
                5 => length = 100,
                6 => length = 108,
                7 => length = 116,
                8 => length = 164,
                _ => return Err(FALSE),
            }
        }
    }
    let waveform = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if waveform > 3 {
        return Err(FALSE);
    }
    let mut values = defaults();
    values[0] = f64::from(waveform) / 3.0;
    if length < 100 {
        values[9] = 0.0;
    }
    if length >= 52 {
        for (i, value) in values
            .iter_mut()
            .enumerate()
            .take(1 + (length - 12) / 8)
            .skip(1)
        {
            let offset = 12 + (i - 1) * 8;
            let plain = f64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap());
            if !valid_plain(i, plain) {
                return Err(FALSE);
            }
            *value = to_normalized(i, plain);
        }
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
    let mut bytes = [0u8; 164];
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

    #[repr(C)]
    struct Stream {
        /// このテストだけで使うIBStream関数表。
        vtable: &'static StreamVTable,
        /// 形式の最大長だけを用意したテスト用領域。
        bytes: [u8; 164],
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
    fn numeric_entry_preserves_values_between_slider_ticks() {
        for (index, text, plain) in [
            (1, "12.345", 12.345),
            (2, "0.125", 0.125),
            (3, "123.456", 123.456),
            (4, "0.4567", 0.4567),
            (5, "0.0625", 0.0625),
            (6, "7", 7.0),
            (7, "25.125", 25.125),
            (8, "32", 32.0),
            (10, "-1234.125", -1234.125),
            (11, "1.2345", 1.2345),
            (13, "-12.345", -12.345),
            (14, "12.345", 12.345),
            (15, "0.125", 0.125),
            (16, "123.456", 123.456),
            (17, "0.4567", 0.4567),
            (18, "0.0625", 0.0625),
            (19, "-1234.125", -1234.125),
        ] {
            let normalized = parse_numeric(index, text).unwrap();
            let parameters = SharedParameters::new();
            parameters.set(index, normalized);
            assert!((to_plain(index, parameters.get(index)) - plain).abs() < 2e-12);
        }
        for (index, min, max) in [
            (1, 0.0, 5000.0),
            (2, 0.0, 5000.0),
            (3, 0.0, 5000.0),
            (4, 0.0, 1.0),
            (5, 0.0, 5000.0),
            (6, 1.0, 7.0),
            (7, 0.0, 100.0),
            (8, 1.0, 32.0),
            (10, -2400.0, 9600.0),
            (11, 0.5, 10.0),
            (13, -48.0, 12.0),
            (14, 0.0, 5000.0),
            (15, 0.0, 5000.0),
            (16, 0.0, 5000.0),
            (17, 0.0, 1.0),
            (18, 0.0, 5000.0),
            (19, -4800.0, 4800.0),
        ] {
            for value in [min, max] {
                assert!(parse_numeric(index, &value.to_string()).is_some());
            }
            for value in [min - 1.0, max + 1.0] {
                assert!(parse_numeric(index, &value.to_string()).is_none());
            }
            for text in ["", "-", "NaN", "inf", "abc"] {
                assert!(parse_numeric(index, text).is_none());
            }
        }
        assert!(parse_numeric(6, "2.5").is_none());
        assert!(parse_numeric(8, "2.5").is_none());
    }

    #[test]
    fn controls_cover_all_steps_and_defaults() {
        let values = defaults();
        assert_eq!(to_plain(9, values[9]), 1.0);
        assert_eq!(to_plain(10, values[10]), 4800.0);
        assert_eq!(to_plain(11, values[11]), 1.0);
        assert_eq!(to_plain(13, values[13]), 0.0);
        assert_eq!(to_plain(19, values[19]), 0.0);
        for i in 0..5 {
            assert_eq!(values[14 + i], values[1 + i]);
        }
        for (index, max) in [(9, 6), (10, 1200), (11, 950), (13, 600), (19, 960)] {
            for tick in 0..=max {
                let plain = slider_to_plain(index, tick);
                let normalized = to_normalized(index, plain);
                assert!(valid_plain(index, plain));
                assert!((to_plain(index, normalized) - plain).abs() < 2e-12);
                assert_eq!((normalized * f64::from(max)).round() as u32, tick);
            }
        }
    }

    #[test]
    fn state_stream_handles_short_transfers_and_rejects_truncation() {
        for waveform in WAVEFORMS {
            let mut stream = Stream {
                vtable: &STREAM_TABLE,
                bytes: [0; 164],
                length: 0,
                position: 0,
            };
            let stream = ptr::from_mut(&mut stream);
            // SAFETY: スタック上のStreamを同じポインターから順に使う。借用を外へ保持しない。
            unsafe {
                let mut values = defaults();
                values[0] = normalized(waveform);
                values[6] = to_normalized(6, 7.0);
                values[7] = to_normalized(7, 37.125);
                values[8] = to_normalized(8, 32.0);
                values[9] = to_normalized(9, 6.0);
                values[10] = to_normalized(10, -1200.125);
                values[11] = to_normalized(11, 5.125);
                values[12] = to_normalized(12, 2.0);
                values[13] = to_normalized(13, -12.345);
                for (i, plain) in [25.0, 15.0, 250.0, 0.4567, 350.25, -1234.125]
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
                for length in 1..164 {
                    (*stream).length = length;
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                }
                // v2は既存値を保持し、追加パラメーターを補う。
                (*stream).bytes[4] = 2;
                (*stream).length = 52;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                for i in 0..6 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                assert_eq!(&legacy[6..9], &defaults()[6..9]);
                assert_eq!(legacy[9], 0.0);
                (*stream).bytes[4] = 3;
                (*stream).length = 68;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                for i in 0..8 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                assert_eq!(legacy[8], defaults()[8]);
                (*stream).bytes[4] = 4;
                (*stream).length = 76;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                assert_eq!(legacy[9], 0.0);
                for i in 0..9 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                (*stream).bytes[4] = 5;
                (*stream).length = 100;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                for i in 0..12 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                assert_eq!(legacy[12], 0.0);
                (*stream).bytes[4] = 6;
                (*stream).length = 108;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                for i in 0..13 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                assert_eq!(to_plain(13, legacy[13]), 0.0);
                (*stream).bytes[4] = 7;
                (*stream).length = 116;
                (*stream).position = 0;
                let legacy = read_state(stream.cast()).unwrap();
                for i in 0..14 {
                    assert!((legacy[i] - values[i]).abs() < 1e-12);
                }
                assert_eq!(&legacy[14..], &defaults()[14..]);
                (*stream).bytes[4] = 8;
                (*stream).length = 164;
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
                    (13, -48.1),
                    (13, 12.1),
                    (13, f64::NAN),
                    (13, f64::INFINITY),
                    (14, -1.0),
                    (15, 5000.1),
                    (16, f64::NAN),
                    (17, -0.1),
                    (17, 1.1),
                    (18, f64::INFINITY),
                    (19, -4800.1),
                    (19, 4800.1),
                    (19, f64::NAN),
                ] {
                    let offset = 12 + (index - 1) * 8;
                    let original: [u8; 8] =
                        (&(*stream).bytes)[offset..offset + 8].try_into().unwrap();
                    (&mut (*stream).bytes)[offset..offset + 8].copy_from_slice(&bad.to_le_bytes());
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                    (&mut (*stream).bytes)[offset..offset + 8].copy_from_slice(&original);
                }
                (*stream).length = 0;
                (*stream).position = 0;
                for (actual, expected) in read_state(stream.cast())
                    .unwrap()
                    .into_iter()
                    .zip(defaults())
                {
                    assert!((actual - expected).abs() < 1e-12);
                }
                (*stream).length = 12;
                (*stream).bytes[4] = 5;
                assert_eq!(read_state(stream.cast()), Err(FALSE));
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
            (0.249, Waveform::Sine),
            (0.25, Waveform::Triangle),
            (0.5, Waveform::Square),
            (0.75, Waveform::Sawtooth),
            (1.0, Waveform::Sawtooth),
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
            let plain = slider_to_plain(6, tick);
            let normalized = to_normalized(6, plain);
            assert_eq!(plain, f64::from(tick + 1));
            assert_eq!(to_plain(6, normalized), plain);
            assert!((canonical(6, normalized) - normalized).abs() < 1e-12);
        }
        for tick in 0..=100 {
            let plain = slider_to_plain(7, tick);
            assert!((to_plain(7, to_normalized(7, plain)) - f64::from(tick)).abs() < 1e-12);
        }
    }

    #[test]
    fn polyphony_integer_control_round_trips_all_values() {
        for tick in 0..=31 {
            let plain = slider_to_plain(8, tick);
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
                assert!((to_plain(i, tick / time_scale()) - ms).abs() < 1e-10);
                assert!((to_normalized(i, ms) * time_scale() - tick).abs() < 1e-10);
            }
            assert_eq!(to_plain(i, 1.0), 5000.0);
            // GUIで選べる全tickの往復変換・単調性・100tickごとの倍率を確認。
            let mut previous = -1.0;
            for tick in 0..=time_scale().floor() as u32 {
                let normalized = f64::from(tick) / time_scale();
                let ms = slider_to_plain(i, tick);
                assert!(ms > previous && ms < 5000.0);
                assert!((to_normalized(i, ms) - normalized).abs() < 1e-12);
                assert!(ms == 0.0 || (to_plain(i, to_normalized(i, ms)) / ms - 1.0).abs() < 1e-12);
                if tick > 100 {
                    let earlier = to_plain(i, f64::from(tick - 100) / time_scale());
                    assert!((ms / earlier - 10.0).abs() < 1e-12);
                }
                previous = ms;
            }
            assert_eq!(slider_to_plain(i, 0), 0.0);
            for (tick, ms) in [(1, 1.0), (101, 10.0), (201, 100.0), (301, 1000.0)] {
                assert!((slider_to_plain(i, tick) / ms - 1.0).abs() < 1e-12);
            }
            assert_eq!(slider_to_plain(i, 371), 5000.0);
            // ホストの連続オートメーションは0〜1ms区間も線形で扱う。
            assert!((to_plain(i, to_normalized(i, 0.5)) - 0.5).abs() < 1e-12);
        }
        assert_eq!(to_plain(4, 0.35), 0.35);
        assert_eq!(to_normalized(4, 0.35), 0.35);
        for tick in 0..=100 {
            let value = slider_to_plain(4, tick);
            assert_eq!((to_normalized(4, value) * 100.0).round() as u32, tick);
        }
    }
}

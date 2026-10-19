// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 波形パラメーターの値変換、VST3変更キューと保存ストリームの読み書き。

use crate::abi::*;
use crate::oscillator::Waveform;
use std::ptr;

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

/// 1回のprocess中だけホストから借用する、波形の変更点のカーソル。
pub struct WaveformPoints<'a> {
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
    pub next: Option<(i32, Waveform)>,
}

impl<'a> WaveformPoints<'a> {
    /// SAFETY: dataの入れ子のインターフェースはホスト所有で、dataの借用期間中有効。
    pub unsafe fn new(data: &'a ProcessData) -> Self {
        let mut result = Self {
            queue: ptr::null_mut(),
            table: None,
            count: 0,
            index: 0,
            previous_offset: 0,
            samples: data.num_samples,
            next: None,
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
                    if (queue_table.get_parameter_id)(queue) == WAVEFORM_ID {
                        result.queue = queue;
                        result.table = Some(queue_table);
                        result.count = (queue_table.get_point_count)(queue);
                        break; // 仕様ではParamIDごとに1キュー。重複時は最初のキューを採用する。
                    }
                }
            }
        }
        result.advance();
        result
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
            if let Some(waveform) = from_normalized(value) {
                self.previous_offset = offset;
                self.next = Some((offset, waveform));
                break;
            }
        }
    }
}

// IBStreamへ渡す音源状態: magic 4バイト、形式バージョンu32、波形番号u32。
// 整数はlittle endian。オシレータの位相・発音中のノートは保存しない。
const STATE_HEADER: [u8; 8] = *b"SVST\x01\0\0\0";

/// SAFETY: streamは呼び出し中有効なホスト所有のIBStream、またはnull。
pub unsafe fn read_state(stream: This) -> Result<Waveform, TResult> {
    if stream.is_null() {
        return Err(INVALID_ARGUMENT);
    }
    // SAFETY: IBStreamの関数表を借用。読み取り領域の長さだけを要求する。
    let table = unsafe { &**stream.cast::<*const StreamVTable>() };
    let mut bytes = [0u8; 12];
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
        // 旧版が保存した空の状態をサイン波として復元。EOFでFALSEを返すストリームも許す。
        if position == 0 && read == 0 && matches!(status, OK | FALSE) {
            return Ok(Waveform::Sine);
        }
        if status != OK || read <= 0 || read as usize > remaining {
            return Err(FALSE);
        }
        position += read as usize;
    }
    if bytes[..8] != STATE_HEADER {
        return Err(FALSE);
    }
    let index = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    WAVEFORMS.get(index as usize).copied().ok_or(FALSE)
}

/// SAFETY: streamは呼び出し中有効なホスト所有のIBStream、またはnull。
pub unsafe fn write_state(stream: This, waveform: Waveform) -> TResult {
    if stream.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: IBStreamの関数表を借用。書き込み関数はバッファを読み取るだけで保持しない。
    let table = unsafe { &**stream.cast::<*const StreamVTable>() };
    let mut bytes = [0u8; 12];
    bytes[..8].copy_from_slice(&STATE_HEADER);
    bytes[8..].copy_from_slice(&waveform_index(waveform).to_le_bytes());
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
        bytes: [u8; 12],
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
            let count = (size as usize).min(2).min(12 - stream.position);
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
    fn state_stream_handles_short_transfers_and_rejects_truncation() {
        for waveform in WAVEFORMS {
            let mut stream = Stream {
                vtable: &STREAM_TABLE,
                bytes: [0; 12],
                length: 0,
                position: 0,
            };
            let stream = ptr::from_mut(&mut stream);
            // SAFETY: スタック上のStreamを同じポインターから順に使う。借用を外へ保持しない。
            unsafe {
                assert_eq!(write_state(stream.cast(), waveform), OK);
                (*stream).position = 0;
                assert_eq!(read_state(stream.cast()), Ok(waveform));
                for length in 1..12 {
                    (*stream).length = length;
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                }
                (*stream).length = 0;
                (*stream).position = 0;
                assert_eq!(read_state(stream.cast()), Ok(Waveform::Sine));
                (*stream).length = 12;
                (*stream).bytes[4] = 2;
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
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! VST3 IBStream上の固定長音色フォーマット。ホストの参照は呼出し中だけ借用する。

use super::{COUNT, to_normalized, to_plain, valid_plain};
use crate::abi::*;

// v37: 固定1076バイト。波形番号u32と、その他の実単位f64を保持する。
const STATE_SIZE: usize = 12 + (COUNT - 1) * 8;
const STATE_HEADER: [u8; 8] = *b"SVST\x25\0\0\0";

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
    use crate::parameters::*;
    use std::ptr;

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
                for version in (0..=37).filter(|v| *v != STATE_HEADER[4]) {
                    (*stream).bytes[4] = version;
                    (*stream).position = 0;
                    assert_eq!(read_state(stream.cast()), Err(FALSE));
                }
                (*stream).bytes[4] = STATE_HEADER[4];
                // ボイス数の範囲外・端数、detuneの範囲外/非有限を拒否。
                for (index, bad) in [
                    (93, 99.0),
                    (98, 4001.0),
                    (104, 2401.0),
                    (95, 0.0),
                    (100, f64::NAN),
                    (106, 95.1),
                    (107, 100.1),
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
                    (31, 2.0),
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
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only
//! 疎なVST3キューの試験用fixture。実ホストの実装・通信記録ではない。
use crate::{abi::*, parameters, synth::Synthesizer};
use std::{io::Write, ptr};
#[repr(C)]
struct Queue {
    table: &'static ParamValueQueueVTable,
    id: u32,
    points: Vec<(i32, f64)>,
}
#[repr(C)]
struct Changes {
    table: &'static ParameterChangesVTable,
    queue: This,
}
unsafe extern "system" fn query(_: This, _: *const [u8; 16], out: *mut This) -> TResult {
    // SAFETY: fixtureのFUnknown呼び出しには有効な出力先を渡す。
    unsafe {
        out.write(ptr::null_mut());
    }
    NO_INTERFACE
}
unsafe extern "system" fn reference(_: This) -> u32 {
    1
}
unsafe extern "system" fn id(this: This) -> u32 {
    // SAFETY: 呼び出し期間中に生存するQueueへのポインター。
    unsafe { (*this.cast::<Queue>()).id }
}
unsafe extern "system" fn count(this: This) -> i32 {
    // SAFETY: 同上。pointsはキュー借用中に変更しない。
    unsafe { (*this.cast::<Queue>()).points.len() as i32 }
}
unsafe extern "system" fn get(
    this: This,
    index: i32,
    offset: *mut i32,
    value: *mut f64,
) -> TResult {
    // SAFETY: Queueと、ABI型に一致する有効な出力先をParameterPointsが渡す。
    unsafe {
        let queue = &*this.cast::<Queue>();
        let Some(&(at, v)) = queue.points.get(index as usize) else {
            return FALSE;
        };
        offset.write(at);
        value.write(v);
    }
    OK
}
unsafe extern "system" fn add(_: This, _: i32, _: f64, _: *mut i32) -> TResult {
    FALSE
}
unsafe extern "system" fn changes_count(_: This) -> i32 {
    1
}
unsafe extern "system" fn changes_get(this: This, index: i32) -> This {
    if index != 0 {
        return ptr::null_mut();
    }
    // SAFETY: ChangesとQueueはParameterPointsの借用より長く生存する。
    unsafe { (*this.cast::<Changes>()).queue }
}
unsafe extern "system" fn changes_add(_: This, _: *const u32, _: *mut i32) -> This {
    ptr::null_mut()
}
static QUEUE: ParamValueQueueVTable = ParamValueQueueVTable {
    unknown: UnknownVTable {
        query_interface: query,
        add_ref: reference,
        release: reference,
    },
    get_parameter_id: id,
    get_point_count: count,
    get_point: get,
    add_point: add,
};
static CHANGES: ParameterChangesVTable = ParameterChangesVTable {
    unknown: UnknownVTable {
        query_interface: query,
        add_ref: reference,
        release: reference,
    },
    get_parameter_count: changes_count,
    get_parameter_data: changes_get,
    add_parameter_data: changes_add,
};
pub fn run(out: &mut impl Write, manifest: &mut impl Write, trace: &mut impl Write) {
    writeln!(trace, "case,sample,normalized,plain").unwrap();
    for parameter in [10, 11] {
        for stride in [1, 16, 64, 256] {
            for mode in ["ramp", "head", "jump"] {
                let name = format!("queue/{mode}/p{parameter}/stride{stride}");
                let mut s = Synthesizer::new();
                s.set_sample_rate(48000.0);
                let mut p = crate::settings(3, 2, 1200.0, 10.0, 32, 7);
                p[7] = 0.25;
                p[parameter] = 0.0;
                s.set_parameters(p);
                for i in 0..32 {
                    s.note_on(crate::note(i, 48 + i as i16), 0.0, 1.0);
                }
                let mut audio = Vec::with_capacity(49152);
                let mut previous = 0.0;
                for start in (0..49152).step_by(stride) {
                    let target = if mode == "jump" {
                        ((start / stride) % 2) as f64
                    } else {
                        {
                            let at = (start + stride) % 8192;
                            at.min(8192 - at) as f64 / 4096.0
                        }
                    };
                    let at = if mode == "head" { 0 } else { stride as i32 - 1 };
                    let mut points = vec![(at, target)];
                    if mode == "jump" && stride > 1 {
                        points.insert(0, (at - 1, previous));
                    }
                    let mut queue = Queue {
                        table: &QUEUE,
                        id: parameters::IDS[parameter],
                        points,
                    };
                    let mut changes = Changes {
                        table: &CHANGES,
                        queue: (&mut queue as *mut Queue).cast(),
                    };
                    // SAFETY: ProcessDataは整数と生ポインターのみ。ゼロ値で不要なバス等を明示する。
                    let mut data: ProcessData = unsafe { std::mem::zeroed() };
                    data.num_samples = stride as i32;
                    data.input_parameter_changes = (&mut changes as *mut Changes).cast();
                    // SAFETY: data、changes、queueはカーソル使用中に生存し、pointsも変更しない。
                    let mut cursor =
                        unsafe { parameters::ParameterPoints::new(&data, parameter, previous) };
                    for offset in 0..stride {
                        let value = cursor.value_at(offset as i32);
                        p[parameter] = value;
                        s.set_parameters(p);
                        audio.push(s.generate_sample());
                        writeln!(
                            trace,
                            "{name},{},{value},{}",
                            start + offset,
                            parameters::to_plain(parameter, value)
                        )
                        .unwrap();
                        previous = value;
                    }
                }
                crate::record(out, manifest, &name, audio.len(), |n| audio[n]);
            }
        }
    }
}

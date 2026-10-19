// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use std::mem::{align_of, offset_of, size_of};

#[test]
fn x86_64_layout() {
    // SDKヘッダ側の同じ値もtests/abi-smoke.cppで独立して検査する。
    assert_eq!(size_of::<FactoryInfo>(), 452);
    assert_eq!(size_of::<ClassInfo>(), 116);
    assert_eq!(size_of::<ClassInfo2>(), 440);
    assert_eq!(size_of::<BusInfo>(), 276);
    assert_eq!(size_of::<RoutingInfo>(), 12);
    assert_eq!(size_of::<ProcessSetup>(), 24);
    assert_eq!(offset_of!(ProcessSetup, sample_rate), 16);
    assert_eq!(size_of::<AudioBusBuffers>(), 24);
    assert_eq!(offset_of!(AudioBusBuffers, silence_flags), 8);
    assert_eq!(offset_of!(AudioBusBuffers, channel_buffers), 16);
    assert_eq!(size_of::<ProcessData>(), 80);
    assert_eq!(align_of::<ProcessData>(), 8);
    assert_eq!(offset_of!(ProcessData, inputs), 24);
    assert_eq!(offset_of!(ProcessData, process_context), 72);
    assert_eq!(size_of::<NoteOnEvent>(), 20);
    assert_eq!(offset_of!(NoteOnEvent, note_id), 16);
    assert_eq!(size_of::<NoteOffEvent>(), 16);
    assert_eq!(offset_of!(NoteOffEvent, note_id), 8);
    assert_eq!(size_of::<Event>(), 48);
    assert_eq!(align_of::<Event>(), 8);
    assert_eq!(offset_of!(Event, event_type), 18);
    assert_eq!(offset_of!(Event, payload), 24);
    assert_eq!(size_of::<EventListVTable>(), 6 * 8);
    assert_eq!(size_of::<FactoryVTable>(), 8 * 8);
    assert_eq!(size_of::<ComponentVTable>(), 14 * 8);
    assert_eq!(size_of::<ProcessorVTable>(), 11 * 8);
}

#[test]
fn rejects_unknown_identifiers_and_clears_output() {
    let factory = GetPluginFactory();
    // SAFETY: 公開関数で取得した有効なFactoryと、型の一致する出力領域を渡す。
    unsafe {
        let table = &**factory.cast::<*const FactoryVTable>();
        let mut out = factory;
        assert_eq!(
            (table.create_instance)(factory, &[0; 16], &COMPONENT_IID, &mut out),
            NO_INTERFACE
        );
        assert!(out.is_null());
        out = factory;
        assert_eq!(
            (table.create_instance)(factory, &COMPONENT_CID, &[0; 16], &mut out),
            NO_INTERFACE
        );
        assert!(out.is_null());
        assert_eq!(
            (table.create_instance)(factory, &COMPONENT_CID, &COMPONENT_IID, ptr::null_mut()),
            INVALID_ARGUMENT
        );
        assert_eq!((table.unknown.release)(factory), 0);
    }
}

#[test]
fn interfaces_share_identity_and_own_the_instance() {
    let factory = GetPluginFactory();
    // SAFETY: 各queryが返す所有参照を保持し、対応するreleaseで一度だけ解放する。
    unsafe {
        let factory_table = &**factory.cast::<*const FactoryVTable>();
        let mut component = ptr::null_mut();
        assert_eq!(
            (factory_table.create_instance)(
                factory,
                &COMPONENT_CID,
                &COMPONENT_IID,
                &mut component
            ),
            OK
        );
        let component_table = &**component.cast::<*const ComponentVTable>();
        let mut processor = ptr::null_mut();
        assert_eq!(
            (component_table.unknown.query_interface)(component, &PROCESSOR_IID, &mut processor),
            OK
        );
        let processor_table = &**processor.cast::<*const ProcessorVTable>();
        let mut identity = ptr::null_mut();
        assert_eq!(
            (processor_table.unknown.query_interface)(processor, &UNKNOWN_IID, &mut identity),
            OK
        );
        assert_eq!(identity, component);
        assert_eq!((component_table.unknown.release)(identity), 2);
        assert_eq!((component_table.unknown.release)(component), 1);
        // ファクトリーとComponentの参照を解放してもProcessorの所有参照は有効。
        assert_eq!((factory_table.unknown.release)(factory), 0);
        assert_eq!((processor_table.get_latency_samples)(processor), 0);
        assert_eq!((processor_table.unknown.release)(processor), 0);
    }
}

// Rust側のホスト模擬。C++の独立検証とは別にMiriでイベント借用・DSP可変状態を検査。
#[repr(C)]
struct TestEvents {
    /// ホストのIEventListを模擬する静的な関数表。
    vtable: &'static EventListVTable,
    /// getEventで返す1件のイベント。各process呼び出しの前にテストが更新する。
    event: Event,
}

unsafe extern "system" fn event_query(_: This, _: *const Tuid, _: *mut This) -> TResult {
    NO_INTERFACE
}
unsafe extern "system" fn event_ref(_: This) -> u32 {
    1
}
unsafe extern "system" fn event_count(_: This) -> i32 {
    1
}
unsafe extern "system" fn event_get(this: This, index: i32, out: *mut Event) -> TResult {
    if index != 0 {
        return INVALID_ARGUMENT;
    }
    // SAFETY: テストが所有するTestEventsから、プラグインのEvent出力領域へコピー。
    unsafe { ptr::copy_nonoverlapping(ptr::addr_of!((*this.cast::<TestEvents>()).event), out, 1) };
    OK
}
unsafe extern "system" fn event_add(_: This, _: *mut Event) -> TResult {
    NOT_IMPLEMENTED
}
static EVENT_TABLE: EventListVTable = EventListVTable {
    unknown: UnknownVTable {
        query_interface: event_query,
        add_ref: event_ref,
        release: event_ref,
    },
    get_event_count: event_count,
    get_event: event_get,
    add_event: event_add,
};

#[test]
fn processes_host_events_and_preserves_phase_through_retrigger() {
    let factory = GetPluginFactory();
    let mut component = ptr::null_mut();
    let mut processor = ptr::null_mut();
    let mut events = TestEvents {
        vtable: &EVENT_TABLE,
        event: Event {
            bus_index: 0,
            sample_offset: 1,
            ppq_position: 0.0,
            flags: 0,
            event_type: 0,
            payload: EventPayload {
                note_on: NoteOnEvent {
                    channel: 0,
                    pitch: 69,
                    tuning: 0.0,
                    velocity: 1.0,
                    length: 0,
                    note_id: 1,
                },
            },
        },
    };
    let mut samples = [9.0_f64; 4];
    let sample_ptr = samples.as_mut_ptr();
    let mut channels = [sample_ptr.cast(), sample_ptr.cast()];
    let events_ptr = ptr::from_mut(&mut events);
    let mut bus = AudioBusBuffers {
        num_channels: 2,
        silence_flags: 0,
        channel_buffers: channels.as_mut_ptr(),
    };
    let mut data = ProcessData {
        process_mode: 0,
        symbolic_sample_size: SAMPLE64,
        num_samples: 2,
        num_inputs: 0,
        num_outputs: 1,
        inputs: ptr::null_mut(),
        outputs: &mut bus,
        input_parameter_changes: ptr::null_mut(),
        output_parameter_changes: ptr::null_mut(),
        input_events: events_ptr.cast(),
        output_events: ptr::null_mut(),
        process_context: ptr::null_mut(),
    };
    // SAFETY: ホストの各領域と所有参照は全呼び出し中有効。processと状態変更は直列に行う。
    unsafe {
        let ft = &**factory.cast::<*const FactoryVTable>();
        assert_eq!(
            (ft.create_instance)(factory, &COMPONENT_CID, &COMPONENT_IID, &mut component),
            OK
        );
        let ct = &**component.cast::<*const ComponentVTable>();
        assert_eq!(
            (ct.unknown.query_interface)(component, &PROCESSOR_IID, &mut processor),
            OK
        );
        let pt = &**processor.cast::<*const ProcessorVTable>();
        assert_eq!((ct.initialize)(component, factory), OK);
        let setup = ProcessSetup {
            process_mode: 0,
            symbolic_sample_size: SAMPLE64,
            max_samples_per_block: 4,
            sample_rate: 1760.0,
        };
        assert_eq!((pt.setup_processing)(processor, &setup), OK);
        assert_eq!((ct.activate_bus)(component, AUDIO, OUTPUT, 0, 1), OK);
        assert_eq!((ct.activate_bus)(component, EVENT, INPUT, 0, 1), OK);
        assert_eq!((ct.set_active)(component, 1), OK);
        assert_eq!((pt.set_processing)(processor, 1), OK);
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert_eq!(sample_ptr.cast::<[f64; 4]>().read(), [0.0, 0.0, 9.0, 9.0]);
        assert_eq!((*data.outputs).silence_flags, STEREO);
        // ブロック先頭で音程を変更しても、前ブロック末尾の位相1/4を継承する。
        (*events_ptr).event.sample_offset = 0;
        (*events_ptr).event.payload.note_on.pitch = 81;
        data.num_samples = 1;
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert!((*sample_ptr - 0.2).abs() < 1e-14);
        (*events_ptr).event.event_type = 1;
        (*events_ptr).event.payload = EventPayload {
            note_off: NoteOffEvent {
                channel: 0,
                pitch: 81,
                velocity: 0.0,
                note_id: 1,
                tuning: 0.0,
            },
        };
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert_eq!(*sample_ptr, 0.0);
        (*events_ptr).event.event_type = 0;
        (*events_ptr).event.payload = EventPayload {
            note_on: NoteOnEvent {
                channel: 0,
                pitch: 69,
                tuning: 0.0,
                velocity: 1.0,
                length: 0,
                note_id: 2,
            },
        };
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert!((*sample_ptr + 0.2).abs() < 1e-14);
        assert_eq!((pt.set_processing)(processor, 0), OK);
        assert_eq!((ct.set_active)(component, 0), OK);
        assert_eq!((ct.terminate)(component), OK);
        (ct.unknown.release)(component);
        assert_eq!((pt.unknown.release)(processor), 0);
        assert_eq!((ft.unknown.release)(factory), 0);
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use super::*;
use std::mem::{align_of, offset_of, size_of};

#[test]
fn x86_64_layout() {
    // SDKヘッダ側の同じ値もtests/abi-smoke.cppで独立して検査する。
    assert_eq!(size_of::<FactoryInfo>(), 452);
    assert_eq!(size_of::<ViewRect>(), 16);
    assert_eq!(size_of::<ViewVTable>(), 15 * 8);
    assert_eq!(size_of::<ComponentHandlerVTable>(), 7 * 8);
    assert_eq!(size_of::<ClassInfo>(), 116);
    assert_eq!(size_of::<ClassInfo2>(), 440);
    assert_eq!(size_of::<BusInfo>(), 276);
    assert_eq!(size_of::<ParameterInfo>(), 792);
    assert_eq!(align_of::<ParameterInfo>(), 8);
    assert_eq!(offset_of!(ParameterInfo, default_normalized_value), 776);
    assert_eq!(offset_of!(ParameterInfo, flags), 788);
    assert_eq!(size_of::<ParameterChangesVTable>(), 6 * 8);
    assert_eq!(size_of::<ParamValueQueueVTable>(), 7 * 8);
    assert_eq!(size_of::<StreamVTable>(), 7 * 8);
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

#[repr(C)]
struct TestPoint {
    /// 借用する1点のIParamValueQueue関数表。
    vtable: &'static ParamValueQueueVTable,
    /// ブロック内の適用位置。
    offset: i32,
    /// ホストから送る正規化値。
    value: f64,
}

#[repr(C)]
struct TestChanges {
    /// 借用するIParameterChanges関数表。
    vtable: &'static ParameterChangesVTable,
    /// この呼び出し中だけ有効なキューへの非所有ポインター。
    queue: This,
}

unsafe extern "system" fn point_id(_: This) -> u32 {
    parameters::WAVEFORM_ID
}
unsafe extern "system" fn point_get(
    this: This,
    index: i32,
    offset: *mut i32,
    value: *mut f64,
) -> TResult {
    assert_eq!(index, 0);
    // SAFETY: 生存中のTestPointと型の一致した出力先をテストが渡す。
    unsafe {
        let point = &*this.cast::<TestPoint>();
        offset.write(point.offset);
        value.write(point.value);
    }
    OK
}
unsafe extern "system" fn point_add(_: This, _: i32, _: f64, _: *mut i32) -> TResult {
    panic!("input queue is borrowed")
}
unsafe extern "system" fn changes_get(this: This, index: i32) -> This {
    assert_eq!(index, 0);
    // SAFETY: このprocess中に有効なTestChangesを借用する。
    unsafe { (*this.cast::<TestChanges>()).queue }
}
unsafe extern "system" fn changes_add(_: This, _: *const u32, _: *mut i32) -> This {
    panic!("input changes are borrowed")
}

static POINT_TABLE: ParamValueQueueVTable = ParamValueQueueVTable {
    unknown: UnknownVTable {
        query_interface: event_query,
        add_ref: event_ref,
        release: event_ref,
    },
    get_parameter_id: point_id,
    get_point_count: event_count,
    get_point: point_get,
    add_point: point_add,
};
static CHANGES_TABLE: ParameterChangesVTable = ParameterChangesVTable {
    unknown: UnknownVTable {
        query_interface: event_query,
        add_ref: event_ref,
        release: event_ref,
    },
    get_parameter_count: event_count,
    get_parameter_data: changes_get,
    add_parameter_data: changes_add,
};

#[test]
fn processes_host_events_and_preserves_phase_through_retrigger() {
    check_host_events(true);
}

#[test]
fn processes_host_events_without_event_bus_activation() {
    check_host_events(false);
}

fn check_host_events(activate_events: bool) {
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
        if activate_events {
            assert_eq!((ct.activate_bus)(component, EVENT, INPUT, 0, 1), OK);
        }
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
        assert!((*sample_ptr - 0.5 * std::f64::consts::SQRT_2).abs() < 1e-14);
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
        assert!((*sample_ptr + 0.5 * std::f64::consts::SQRT_2).abs() < 1e-14);
        assert_eq!((pt.set_processing)(processor, 0), OK);
        assert_eq!((ct.set_active)(component, 0), OK);
        // 明示的な無効化は再開しても維持され、届いたNote Onを処理しない。
        assert_eq!((ct.activate_bus)(component, EVENT, INPUT, 0, 0), OK);
        assert_eq!((ct.initialize)(component, factory), FALSE);
        assert_eq!((ct.set_active)(component, 1), OK);
        assert_eq!((pt.set_processing)(processor, 1), OK);
        data.num_samples = 4;
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert_eq!(sample_ptr.cast::<[f64; 4]>().read(), [0.0; 4]);
        assert_eq!((*data.outputs).silence_flags, STEREO);
        assert_eq!((pt.set_processing)(processor, 0), OK);
        assert_eq!((ct.set_active)(component, 0), OK);
        assert_eq!((ct.terminate)(component), OK);
        // 同一インスタンスの再初期化でも、通知なしで再びノートを受け付ける。
        assert_eq!((ct.initialize)(component, factory), OK);
        assert_eq!((pt.setup_processing)(processor, &setup), OK);
        assert_eq!((ct.activate_bus)(component, AUDIO, OUTPUT, 0, 1), OK);
        assert_eq!((ct.set_active)(component, 1), OK);
        assert_eq!((pt.set_processing)(processor, 1), OK);
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert!(
            sample_ptr
                .cast::<[f64; 4]>()
                .read()
                .iter()
                .any(|x| x.abs() > 0.5)
        );
        assert_eq!((*data.outputs).silence_flags, 0);
        // 借用キュー経由で矩形波へ変更し、Miriでも入れ子の生ポインターを検査する。
        let mut point = TestPoint {
            vtable: &POINT_TABLE,
            offset: 0,
            value: 2.0 / 3.0,
        };
        let mut changes = TestChanges {
            vtable: &CHANGES_TABLE,
            queue: ptr::from_mut(&mut point).cast(),
        };
        data.input_parameter_changes = ptr::from_mut(&mut changes).cast();
        data.input_events = ptr::null_mut();
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert!(
            sample_ptr
                .cast::<[f64; 4]>()
                .read()
                .iter()
                .all(|x| x.abs() == 0.5)
        );
        assert_eq!((pt.set_processing)(processor, 0), OK);
        data.num_samples = 0;
        data.num_outputs = 0;
        data.outputs = ptr::null_mut();
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert_eq!((ct.set_active)(component, 0), OK);
        assert_eq!((ct.terminate)(component), OK);
        (ct.unknown.release)(component);
        assert_eq!((pt.unknown.release)(processor), 0);
        assert_eq!((ft.unknown.release)(factory), 0);
    }
}

#[repr(C)]
struct GuiHandler {
    /// ホストが提供するIComponentHandler関数表。
    table: &'static ComponentHandlerVTable,
    /// テスト側1参照にプラグインの所有参照を加えた数。
    refs: std::cell::Cell<u32>,
    /// 再入テスト用のController借用参照。
    controller: This,
    /// begin/perform/endの順序を10進数字で記録する。
    sequence: std::cell::Cell<u32>,
}
unsafe extern "system" fn gui_retain(this: This) -> u32 {
    // SAFETY: テスト所有のGuiHandler。UIスレッドで直列に操作する。
    let h = unsafe { &*this.cast::<GuiHandler>() };
    h.refs.set(h.refs.get() + 1);
    h.refs.get()
}
unsafe extern "system" fn gui_release(this: This) -> u32 {
    // SAFETY: テストの所有参照は解放せず、プラグインの追加参照だけを回収する。
    let h = unsafe { &*this.cast::<GuiHandler>() };
    assert!(h.refs.get() > 1);
    h.refs.set(h.refs.get() - 1);
    h.refs.get()
}
unsafe extern "system" fn gui_begin(this: This, id: u32) -> TResult {
    assert_eq!(id, parameters::WAVEFORM_ID);
    // SAFETY: テストが保持するハンドラーの記録領域。
    unsafe { &*this.cast::<GuiHandler>() }.sequence.set(1);
    OK
}
unsafe extern "system" fn gui_perform(this: This, id: u32, value: f64) -> TResult {
    // SAFETY: Controllerへの参照はテストが保持している。ホストからの同期的再入を模擬。
    unsafe {
        let h = &*this.cast::<GuiHandler>();
        h.sequence.set(h.sequence.get() * 10 + 2);
        let table = &**h.controller.cast::<*const ControllerVTable>();
        assert_eq!((table.set_param_normalized)(h.controller, id, value), OK);
        assert_eq!(
            (table.set_component_handler)(h.controller, ptr::null_mut()),
            OK
        );
    }
    OK
}
unsafe extern "system" fn gui_end(this: This, _: u32) -> TResult {
    // SAFETY: performEditで解除されても、編集処理の所有参照がendEditまで保持する。
    let h = unsafe { &*this.cast::<GuiHandler>() };
    h.sequence.set(h.sequence.get() * 10 + 3);
    OK
}
unsafe extern "system" fn gui_restart(_: This, _: i32) -> TResult {
    NOT_IMPLEMENTED
}
static GUI_HANDLER: ComponentHandlerVTable = ComponentHandlerVTable {
    unknown: UnknownVTable {
        query_interface: event_query,
        add_ref: gui_retain,
        release: gui_release,
    },
    begin_edit: gui_begin,
    perform_edit: gui_perform,
    end_edit: gui_end,
    restart_component: gui_restart,
};

#[test]
fn gui_gesture_keeps_handler_alive_during_reentrant_replacement() {
    let factory = GetPluginFactory();
    // SAFETY: 取得したControllerとスタック上のGuiHandlerをテスト中保持する。
    unsafe {
        let ft = &**factory.cast::<*const FactoryVTable>();
        let mut controller = ptr::null_mut();
        assert_eq!(
            (ft.create_instance)(factory, &COMPONENT_CID, &CONTROLLER_IID, &mut controller),
            OK
        );
        let ct = &**controller.cast::<*const ControllerVTable>();
        let mut h = GuiHandler {
            table: &GUI_HANDLER,
            refs: std::cell::Cell::new(1),
            controller,
            sequence: std::cell::Cell::new(0),
        };
        let handler = ptr::from_mut(&mut h);
        assert_eq!((ct.set_component_handler)(controller, handler.cast()), OK);
        assert_eq!((*handler).refs.get(), 2);
        assert_eq!(
            (ct.set_param_normalized)(controller, parameters::WAVEFORM_ID, 0.0),
            OK
        );
        assert_eq!((*handler).sequence.get(), 0);
        assert_eq!(plugin::edit_waveform(controller, 3), OK);
        assert_eq!((*handler).sequence.get(), 123);
        assert_eq!((*handler).refs.get(), 1);
        assert_eq!(
            (ct.get_param_normalized)(controller, parameters::WAVEFORM_ID),
            1.0
        );
        assert_eq!(plugin::edit_waveform(controller, 0), FALSE);
        assert_eq!((ct.unknown.release)(controller), 0);
        assert_eq!((ft.unknown.release)(factory), 0);
    }
}

#[test]
fn editor_owns_controller_and_has_fixed_size() {
    let factory = GetPluginFactory();
    // SAFETY: 画面をattachせずABI・参照管理・固定サイズだけを検証する。OS APIは呼ばない。
    unsafe {
        let ft = &**factory.cast::<*const FactoryVTable>();
        let mut controller = ptr::null_mut();
        assert_eq!(
            (ft.create_instance)(factory, &COMPONENT_CID, &CONTROLLER_IID, &mut controller),
            OK
        );
        let ct = &**controller.cast::<*const ControllerVTable>();
        assert!((ct.create_view)(controller, c"unknown".as_ptr().cast()).is_null());
        let view = (ct.create_view)(controller, c"editor".as_ptr().cast());
        assert!(!view.is_null());
        let vt = &**view.cast::<*const ViewVTable>();
        assert_eq!((ct.unknown.release)(controller), 1);
        let mut identity = ptr::null_mut();
        assert_eq!(
            (vt.unknown.query_interface)(view, &UNKNOWN_IID, &mut identity),
            OK
        );
        assert_eq!(view, identity);
        assert_eq!((vt.unknown.release)(identity), 1);
        let mut rect = ViewRect {
            left: 0,
            top: 0,
            right: 0,
            bottom: 0,
        };
        assert_eq!((vt.get_size)(view, &mut rect), OK);
        assert_eq!((rect.right, rect.bottom), (360, 180));
        rect.right = 720;
        assert_eq!((vt.on_size)(view, &rect), FALSE);
        assert_eq!((vt.check_size_constraint)(view, &mut rect), OK);
        assert_eq!((rect.right, rect.bottom), (360, 180));
        assert_eq!((vt.on_size)(view, &rect), OK);
        assert_eq!((vt.removed)(view), OK);
        assert_eq!((vt.unknown.release)(view), 0);
        assert_eq!((ft.unknown.release)(factory), 0);
    }
}

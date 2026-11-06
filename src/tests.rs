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
    assert_eq!(size_of::<UnitInfo>(), 268);
    assert_eq!(align_of::<UnitInfo>(), 4);
    assert_eq!(offset_of!(UnitInfo, program_list_id), 264);
    assert_eq!(size_of::<ProgramListInfo>(), 264);
    assert_eq!(offset_of!(ProgramListInfo, program_count), 260);
    assert_eq!(size_of::<UnitInfoVTable>(), 15 * 8);
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
        assert_eq!((processor_table.get_latency_samples)(processor), 1);
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
    /// 変更対象のパラメーター。
    id: u32,
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

unsafe extern "system" fn point_id(this: This) -> u32 {
    // SAFETY: 呼び出し中有効なテストのキュー。
    unsafe { (*this.cast::<TestPoint>()).id }
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
    let mut bypass = TestPoint {
        vtable: &POINT_TABLE,
        id: parameters::IDS[9],
        offset: 0,
        value: 0.0,
    };
    let mut bypass_changes = TestChanges {
        vtable: &CHANGES_TABLE,
        queue: ptr::from_mut(&mut bypass).cast(),
    };
    let mut data = ProcessData {
        process_mode: 0,
        symbolic_sample_size: SAMPLE64,
        num_samples: 2,
        num_inputs: 0,
        num_outputs: 1,
        inputs: ptr::null_mut(),
        outputs: &mut bus,
        input_parameter_changes: ptr::from_mut(&mut bypass_changes).cast(),
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
            sample_rate: 56_320.0, // 440 Hzの1サンプルあたりの位相は1/128周期。
        };
        for sample_rate in [44_100.0, 48_000.0, 96_000.0, 192_000.0] {
            let accepted = ProcessSetup {
                sample_rate,
                ..setup
            };
            assert_eq!((pt.setup_processing)(processor, &accepted), OK);
        }
        assert_eq!((pt.setup_processing)(processor, &setup), OK);
        for sample_rate in [
            f64::NAN,
            f64::INFINITY,
            f64::NEG_INFINITY,
            -1.0,
            0.0,
            32_000.0,
            44_100.0_f64.next_down(),
        ] {
            let rejected = ProcessSetup {
                sample_rate,
                ..setup
            };
            assert_eq!(
                (pt.setup_processing)(processor, &rejected),
                INVALID_ARGUMENT
            );
        }
        assert_eq!((pt.get_tail_samples)(processor), u32::MAX);
        assert_eq!((ct.activate_bus)(component, AUDIO, OUTPUT, 0, 1), OK);
        if activate_events {
            assert_eq!((ct.activate_bus)(component, EVENT, INPUT, 0, 1), OK);
        }
        assert_eq!((ct.set_active)(component, 1), OK);
        assert_eq!((pt.set_processing)(processor, 1), OK);
        let before_mode = crate::float_mode::read();
        assert_eq!((pt.process)(processor, &mut data), OK);
        assert_eq!(crate::float_mode::read(), before_mode);
        assert_eq!(sample_ptr.cast::<[f64; 4]>().read(), [0.0, 0.0, 9.0, 9.0]);
        assert_eq!((*data.outputs).silence_flags, STEREO);
        // レート拒否後も直前の設定を維持。音程変更時は末尾の位相1/128を継承する。
        (*events_ptr).event.sample_offset = 0;
        (*events_ptr).event.payload.note_on.pitch = 81;
        data.num_samples = 1;
        assert_eq!((pt.process)(processor, &mut data), OK);
        let step = 1000.0 / setup.sample_rate / 10.0;
        let sine = |position: f64| (std::f64::consts::TAU * position / 128.0).sin();
        assert_eq!(*sample_ptr, 0.0);
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
        let release_start = 0.5 * (2.0 * step - step * step);
        assert!((*sample_ptr - 0.5 * step * std::f64::consts::SQRT_2 * sine(1.0)).abs() < 1e-14);
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
        let restart = release_start * (1.0 - 1000.0 / setup.sample_rate / 100.0);
        assert!((*sample_ptr - release_start * std::f64::consts::SQRT_2 * sine(3.0)).abs() < 1e-14);
        data.input_events = ptr::null_mut();
        data.input_parameter_changes = ptr::null_mut();
        // 1サンプル前の再トリガー開始値を確認し、その後も位相を進める。
        for n in 0..69 {
            assert_eq!((pt.process)(processor, &mut data), OK);
            if n == 0 {
                assert!(
                    (*sample_ptr - restart * std::f64::consts::SQRT_2 * sine(5.0)).abs() < 1e-14
                );
            }
            assert!(sample_ptr.read().is_finite());
        }
        data.input_events = events_ptr.cast();
        data.input_parameter_changes = ptr::from_mut(&mut bypass_changes).cast();
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
        let peak = 0.5 * step * std::f64::consts::SQRT_2;
        let expected_sine = [0.0, 0.0, peak * sine(76.0), 2.0 * peak * sine(77.0)];
        for (actual, expected) in sample_ptr
            .cast::<[f64; 4]>()
            .read()
            .into_iter()
            .zip(expected_sine)
        {
            assert!((actual - expected).abs() < 1e-14);
        }
        assert_eq!((*data.outputs).silence_flags, 0);
        // 借用キュー経由で矩形波へ変更し、Miriでも入れ子の生ポインターを検査する。
        let mut point = TestPoint {
            vtable: &POINT_TABLE,
            id: parameters::WAVEFORM_ID,
            offset: 0,
            value: 0.5,
        };
        let mut changes = TestChanges {
            vtable: &CHANGES_TABLE,
            queue: ptr::from_mut(&mut point).cast(),
        };
        data.input_parameter_changes = ptr::from_mut(&mut changes).cast();
        data.input_events = ptr::null_mut();
        assert_eq!((pt.process)(processor, &mut data), OK);
        let square = |n: usize| {
            crate::oscillator::reference::audio_sample(
                crate::oscillator::Waveform::Square,
                (n + 75) as f64 / 128.0,
                1.0 / 128.0,
            ) * n as f64
                * 0.5
                * step
        };
        let expected = [3.0 * peak * sine(78.0), square(4), square(5), square(6)];
        for (actual, expected) in sample_ptr
            .cast::<[f64; 4]>()
            .read()
            .into_iter()
            .zip(expected)
        {
            assert!((actual - expected).abs() < 1e-7);
        }
        data.num_samples = 1;
        data.input_parameter_changes = ptr::null_mut();
        for n in 7..76 {
            assert_eq!((pt.process)(processor, &mut data), OK);
            assert!((sample_ptr.read() - square(n)).abs() < 1e-7);
        }
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
        assert_eq!(plugin::edit_parameter(controller, 0, 1.0), OK);
        assert_eq!((*handler).sequence.get(), 123);
        assert_eq!((*handler).refs.get(), 1);
        assert_eq!(
            (ct.get_param_normalized)(controller, parameters::WAVEFORM_ID),
            1.0
        );
        assert_eq!(plugin::edit_parameter(controller, 0, 0.0), FALSE);
        assert_eq!((ct.set_component_handler)(controller, handler.cast()), OK);
        (*handler).sequence.set(0);
        // 配送前の再描画で同じ値が複数回キューへ入っても、編集通知は重複させない。
        assert_eq!(plugin::edit_parameter(controller, 0, 1.0), OK);
        assert_eq!(plugin::edit_parameter(controller, 0, 1.0), OK);
        assert_eq!((*handler).sequence.get(), 0);
        assert_eq!((*handler).refs.get(), 2);
        // 異なる値へ戻す編集は通知する。
        assert_eq!(plugin::edit_parameter(controller, 0, 0.0), OK);
        assert_eq!((*handler).sequence.get(), 123);
        assert_eq!((*handler).refs.get(), 1);
        // ドラッグ中にホストがhandlerを交換しても、開始した相手へ最後まで通知する。
        assert_eq!((ct.set_component_handler)(controller, handler.cast()), OK);
        let gesture = plugin::begin_parameter_edit(controller, 0).unwrap();
        assert_eq!((*handler).refs.get(), 3);
        assert_eq!(
            plugin::perform_parameter_edit(controller, &gesture, 0, 0.5),
            OK
        );
        assert_eq!((*handler).refs.get(), 2);
        assert_eq!(
            plugin::perform_parameter_edit(controller, &gesture, 0, 1.0),
            OK
        );
        plugin::end_parameter_edit(&gesture, 0);
        assert_eq!((*handler).sequence.get(), 1223);
        drop(gesture);
        assert_eq!((*handler).refs.get(), 1);
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
        assert_eq!((rect.right, rect.bottom), (1277, 672));
        rect.right = 800;
        assert_eq!((vt.on_size)(view, &rect), FALSE);
        assert_eq!((vt.check_size_constraint)(view, &mut rect), OK);
        assert_eq!((rect.right, rect.bottom), (1277, 672));
        assert_eq!((vt.on_size)(view, &rect), OK);
        assert_eq!((vt.removed)(view), OK);
        assert_eq!((vt.unknown.release)(view), 0);
        assert_eq!((ft.unknown.release)(factory), 0);
    }
}

#[test]
fn parameter_queues_are_enumerated_once_and_keep_the_first_duplicate() {
    #[repr(C)]
    struct Queues {
        table: &'static ParameterChangesVTable,
        entries: [This; 6],
        visits: usize,
    }
    unsafe extern "system" fn count(_: This) -> i32 {
        6
    }
    unsafe extern "system" fn get(this: This, at: i32) -> This {
        // SAFETY: このテストの配列を逐次読み、列挙回数だけを記録する。
        let queues = unsafe { &mut *this.cast::<Queues>() };
        queues.visits += 1;
        queues.entries[at as usize]
    }
    unsafe extern "system" fn no_points(_: This) -> i32 {
        0
    }
    static MULTIPLE: ParameterChangesVTable = ParameterChangesVTable {
        unknown: UnknownVTable {
            query_interface: event_query,
            add_ref: event_ref,
            release: event_ref,
        },
        get_parameter_count: count,
        get_parameter_data: get,
        add_parameter_data: changes_add,
    };
    static EMPTY: ParamValueQueueVTable = ParamValueQueueVTable {
        unknown: UnknownVTable {
            query_interface: event_query,
            add_ref: event_ref,
            release: event_ref,
        },
        get_parameter_id: point_id,
        get_point_count: no_points,
        get_point: point_get,
        add_point: point_add,
    };
    let mut first = TestPoint {
        vtable: &POINT_TABLE,
        id: parameters::IDS[11],
        offset: 0,
        value: 0.4,
    };
    let mut duplicate = TestPoint {
        vtable: &POINT_TABLE,
        id: parameters::IDS[11],
        offset: 0,
        value: 0.9,
    };
    let mut empty = TestPoint {
        vtable: &EMPTY,
        id: parameters::IDS[32],
        offset: 0,
        value: 0.9,
    };
    let mut later = TestPoint {
        vtable: &POINT_TABLE,
        id: parameters::IDS[32],
        offset: 0,
        value: 0.9,
    };
    let mut unknown = TestPoint {
        vtable: &POINT_TABLE,
        id: u32::MAX,
        offset: 0,
        value: 0.9,
    };
    let mut queues = Queues {
        table: &MULTIPLE,
        entries: [
            ptr::from_mut(&mut first).cast(),
            ptr::null_mut(),
            ptr::from_mut(&mut empty).cast(),
            ptr::from_mut(&mut duplicate).cast(),
            ptr::from_mut(&mut later).cast(),
            ptr::from_mut(&mut unknown).cast(),
        ],
        visits: 0,
    };
    // SAFETY: ProcessDataは整数とnull許容の生ポインターだけで、このスコープのキューを借用する。
    let mut data: ProcessData = unsafe { std::mem::zeroed() };
    data.num_samples = 1;
    data.input_parameter_changes = ptr::from_mut(&mut queues).cast();
    let mut values = parameters::defaults();
    values[32] = 0.2;
    let mut points = unsafe { parameters::ParameterPoints::all(&data, &values) };
    assert_eq!(queues.visits, 6);
    assert_eq!(points[11].value_at(0), 0.4);
    assert_eq!(points[32].value_at(0), 0.2);
    assert_eq!(points.iter().filter(|p| p.automated).count(), 1);
}

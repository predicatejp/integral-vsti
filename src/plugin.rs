// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

use crate::abi::*;
use crate::host::HostRef;
use crate::parameters::{self, IDS, NAMES, ParameterPoints, SharedParameters};
use crate::synth::Synthesizer;
use crate::voice::Note;

use std::cell::{RefCell, UnsafeCell};
use std::ffi::c_void;
use std::mem::MaybeUninit;
use std::ptr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

mod programs;

// vtableポインターを先頭に置く。ownerはホストに見えない内部データ。
#[repr(C)]
struct Interface {
    /// 対応するVST3インターフェースの関数表。ホストが参照するため先頭に配置する。
    vtable: *const c_void,
    /// このInterfaceを埋め込んでいるPluginへの非所有ポインター。最後のreleaseまで有効。
    owner: *mut Plugin,
}

struct Plugin {
    /// バス構成・初期化・状態保存を提供するIComponentの公開口。
    component: Interface,
    /// 処理設定と波形生成を提供するIAudioProcessorの公開口。
    processor: Interface,
    /// パラメーター照会等を提供するIEditControllerの公開口。独立インスタンスは持たない。
    controller: Interface,
    /// 必要なホストの時間情報を返すIProcessContextRequirementsの公開口。
    context_requirements: Interface,
    /// ルートユニットと内蔵プログラム一覧を提供するIUnitInfo。
    unit_info: Interface,
    /// 全インターフェースで共有する所有参照数。0になったときPlugin全体を破棄する。
    refs: AtomicU32,
    /// initializeが成功してからterminateまでtrueとなる初期化状態。
    initialized: AtomicBool,
    /// setActiveで設定される音源インスタンスの有効状態。
    active: AtomicBool,
    /// setProcessingで設定される音声処理状態。falseの間は正のサンプル数を処理しない。
    processing: AtomicBool,
    /// 音声出力バス0の有効状態。falseの間は出力バッファへ書き込まない。
    output_active: AtomicBool,
    /// イベント入力バス0の有効状態。falseならprocessで消音し、イベントを受け付けない。
    /// initialize時は互換性のためtrueとし、その後はactivateBusの明示的な指定に従う。
    event_active: AtomicBool,
    /// DSPへ適用済み、またはsetStateから次のprocessへ渡す22パラメーター。保存時にもatomicで読む。
    processor_parameters: SharedParameters,
    /// Controllerが表示する22パラメーター。ホストの値通知とComponentの状態復元で更新する。
    controller_parameters: Arc<SharedParameters>,
    /// UIスレッドだけで設定・呼び出し・解放するIComponentHandlerの所有参照。
    component_handler: RefCell<Option<HostRef>>,
    /// オシレータ・現在のノート・サンプルレートを所有する可変DSP状態。
    /// processとライフサイクルの更新はVST3規約に従いホストが直列化する。
    /// Controller・照会APIからは触れない。オーディオスレッドでロックしない。
    synth: UnsafeCell<Synthesizer>,
    /// 設定済みの最大ブロック長（サンプル数）。初期化前・終了後は0。
    max_samples: AtomicI32,
    /// 設定済みのサンプル形式（SAMPLE32またはSAMPLE64）。正の長さの処理には一致を要求する。
    sample_size: AtomicI32,
}

const UNKNOWN: UnknownVTable = UnknownVTable {
    query_interface: query,
    add_ref,
    release,
};

static COMPONENT: ComponentVTable = ComponentVTable {
    unknown: UNKNOWN,
    initialize,
    terminate,
    get_controller_class_id: controller_id,
    set_io_mode,
    get_bus_count: bus_count,
    get_bus_info: bus_info,
    get_routing_info: routing_info,
    activate_bus,
    set_active,
    set_state: set_processor_state,
    get_state: get_processor_state,
};
static PROCESSOR: ProcessorVTable = ProcessorVTable {
    unknown: UNKNOWN,
    set_bus_arrangements,
    get_bus_arrangement,
    can_process_sample_size,
    get_latency_samples: zero,
    setup_processing,
    set_processing,
    process,
    get_tail_samples,
};
static CONTEXT_REQUIREMENTS: ContextRequirementsVTable = ContextRequirementsVTable {
    unknown: UNKNOWN,
    get_process_context_requirements: zero,
};
// eguiとホストのパラメーター操作が同じVST3パラメーターを扱う。
static CONTROLLER: ControllerVTable = ControllerVTable {
    unknown: UNKNOWN,
    initialize,
    terminate,
    set_component_state: set_controller_component_state,
    set_state: empty_state,
    get_state: empty_state,
    get_parameter_count: parameter_count,
    get_parameter_info: parameter_info,
    get_param_string_by_value: parameter_string,
    get_param_value_by_string: parameter_value,
    normalized_param_to_plain: normalized_to_plain,
    plain_param_to_normalized: plain_to_normalized,
    get_param_normalized: parameter_normalized,
    set_param_normalized: set_parameter,
    set_component_handler,
    create_view,
};

pub unsafe fn create(iid: &Tuid, out: *mut This) -> TResult {
    if !matches!(
        *iid,
        UNKNOWN_IID
            | PLUGIN_BASE_IID
            | COMPONENT_IID
            | PROCESSOR_IID
            | CONTROLLER_IID
            | CONTEXT_REQUIREMENTS_IID
            | UNIT_INFO_IID
    ) {
        return NO_INTERFACE;
    }
    let plugin = Box::new(Plugin {
        component: Interface {
            vtable: ptr::from_ref(&COMPONENT).cast(),
            owner: ptr::null_mut(),
        },
        processor: Interface {
            vtable: ptr::from_ref(&PROCESSOR).cast(),
            owner: ptr::null_mut(),
        },
        controller: Interface {
            vtable: ptr::from_ref(&CONTROLLER).cast(),
            owner: ptr::null_mut(),
        },
        context_requirements: Interface {
            vtable: ptr::from_ref(&CONTEXT_REQUIREMENTS).cast(),
            owner: ptr::null_mut(),
        },
        unit_info: Interface {
            vtable: ptr::from_ref(&programs::UNIT_INFO).cast(),
            owner: ptr::null_mut(),
        },
        refs: AtomicU32::new(1),
        initialized: AtomicBool::new(false),
        active: AtomicBool::new(false),
        processing: AtomicBool::new(false),
        output_active: AtomicBool::new(false),
        event_active: AtomicBool::new(false),
        processor_parameters: SharedParameters::new(),
        controller_parameters: Arc::new(SharedParameters::new()),
        component_handler: RefCell::new(None),
        synth: UnsafeCell::new(Synthesizer::new()),
        max_samples: AtomicI32::new(0),
        sample_size: AtomicI32::new(SAMPLE32),
    });
    let owner = Box::into_raw(plugin);
    // SAFETY: 公開前のBoxを生ポインター経由で初期化する。以後ownerと各Interfaceの
    // アドレスは最後のreleaseまで不変。初期参照をホストへ移す。
    unsafe {
        (*owner).component.owner = owner;
        (*owner).processor.owner = owner;
        (*owner).controller.owner = owner;
        (*owner).context_requirements.owner = owner;
        (*owner).unit_info.owner = owner;
        out.write(interface(&*owner, iid));
    }
    OK
}

fn interface(plugin: &Plugin, iid: &Tuid) -> This {
    let target = match *iid {
        UNKNOWN_IID | PLUGIN_BASE_IID | COMPONENT_IID => &plugin.component,
        PROCESSOR_IID => &plugin.processor,
        CONTROLLER_IID => &plugin.controller,
        CONTEXT_REQUIREMENTS_IID => &plugin.context_requirements,
        UNIT_INFO_IID => &plugin.unit_info,
        _ => return ptr::null_mut(),
    };
    ptr::from_ref(target).cast_mut().cast()
}

// SAFETY契約: thisは公開済みInterfaceで、呼び出し元が所有参照を持つ。
// 生存中のownerだけを返す。共有状態はatomic、直列化されたDSP状態はUnsafeCellを用いる。
unsafe fn instance<'a>(this: This) -> &'a Plugin {
    unsafe { &*(*this.cast::<Interface>()).owner }
}

unsafe extern "system" fn query(this: This, iid: *const Tuid, out: *mut This) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outは有効な出力領域、iidはnullでなければ16バイトの識別子。
    unsafe {
        out.write(ptr::null_mut());
        if iid.is_null() {
            return INVALID_ARGUMENT;
        }
        let plugin = instance(this);
        let result = interface(plugin, &*iid);
        if result.is_null() {
            return NO_INTERFACE;
        }
        crate::retain(&plugin.refs);
        out.write(result);
    }
    OK
}

unsafe extern "system" fn add_ref(this: This) -> u32 {
    // SAFETY: thisの所有参照が呼び出しの間ownerを保持する。
    crate::retain(unsafe { &instance(this).refs })
}

unsafe extern "system" fn release(this: This) -> u32 {
    // SAFETY: thisは生存中のInterface。最後の参照だけがownerを解放する。
    unsafe {
        let owner = (*this.cast::<Interface>()).owner;
        let remaining = (*owner).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if remaining == 0 {
            drop(Box::from_raw(owner));
        }
        remaining
    }
}

unsafe extern "system" fn initialize(this: This, context: This) -> TResult {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: thisは生存中のInterface。contextは利用・保持しない。
    let plugin = unsafe { instance(this) };
    if plugin.initialized.swap(true, Ordering::Relaxed) {
        FALSE
    } else {
        // 仕様逸脱を伴う互換性対応: VST3では全バスの初期状態は無効で、
        // kDefaultActiveもホストへのactivateBus呼び出し要求にすぎない。
        // https://steinbergmedia.github.io/vst3_doc/vstinterfaces/group__vstBus.html
        // ただしイベント入力の有効化通知を省略するホストでもノートを受信できるよう、
        // この固定のデフォルト有効イベントバスだけは初期化時に有効とする。
        // 以後の明示的な無効化は尊重し、setActive/setProcessingでは再有効化しない。
        // 音声出力バスにはこの例外を適用しない。
        plugin.event_active.store(true, Ordering::Relaxed);
        OK
    }
}

unsafe extern "system" fn terminate(this: This) -> TResult {
    // SAFETY: thisは生存中のInterface。終了はホストが処理停止後に呼ぶ。
    let plugin = unsafe { instance(this) };
    if !plugin.initialized.swap(false, Ordering::Relaxed) {
        return FALSE;
    }
    plugin.processing.store(false, Ordering::Relaxed);
    plugin.active.store(false, Ordering::Relaxed);
    plugin.output_active.store(false, Ordering::Relaxed);
    plugin.max_samples.store(0, Ordering::Relaxed);
    plugin.event_active.store(false, Ordering::Relaxed);
    // SAFETY: terminateはprocessの終了後に呼ばれる。
    unsafe { (*plugin.synth.get()).stop() };
    let old_handler = plugin.component_handler.replace(None);
    drop(old_handler);
    OK
}

unsafe extern "system" fn controller_id(_: This, out: *mut Tuid) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outは16バイトの書き込み領域。Controllerは同一オブジェクトのqueryで提供する。
    unsafe {
        out.write([0; 16]);
    }
    FALSE
}

unsafe extern "system" fn set_io_mode(_: This, mode: i32) -> TResult {
    if (0..=2).contains(&mode) {
        OK
    } else {
        INVALID_ARGUMENT
    }
}

fn has_bus(media: i32, direction: i32) -> bool {
    matches!((media, direction), (AUDIO, OUTPUT) | (EVENT, INPUT))
}

unsafe extern "system" fn bus_count(_: This, media: i32, direction: i32) -> i32 {
    i32::from(has_bus(media, direction))
}

unsafe extern "system" fn bus_info(
    _: This,
    media: i32,
    direction: i32,
    index: i32,
    out: *mut BusInfo,
) -> TResult {
    if out.is_null() || index != 0 || !has_bus(media, direction) {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはBusInfoの書き込み領域。バス構成は固定。
    unsafe {
        out.write(BusInfo {
            media_type: media,
            direction,
            channel_count: if media == AUDIO { 2 } else { 16 },
            name: utf16(if media == AUDIO {
                "Stereo Out"
            } else {
                "Event In"
            }),
            bus_type: 0,
            flags: 1,
        });
    }
    OK
}

unsafe extern "system" fn routing_info(
    _: This,
    _: *mut RoutingInfo,
    _: *mut RoutingInfo,
) -> TResult {
    NOT_IMPLEMENTED
}

unsafe extern "system" fn activate_bus(
    this: This,
    media: i32,
    direction: i32,
    index: i32,
    state: u8,
) -> TResult {
    if index != 0 || !has_bus(media, direction) {
        return INVALID_ARGUMENT;
    }
    if media == AUDIO {
        // SAFETY: thisは生存中のInterface。
        unsafe { instance(this) }
            .output_active
            .store(state != 0, Ordering::Relaxed);
    } else {
        // SAFETY: thisは生存中のInterface。DSPへの反映はprocess内で行う。
        unsafe { instance(this) }
            .event_active
            .store(state != 0, Ordering::Relaxed);
    }
    OK
}

unsafe extern "system" fn set_active(this: This, state: u8) -> TResult {
    // SAFETY: thisは生存中のInterface。状態遷移はホストが直列化する。
    let plugin = unsafe { instance(this) };
    if !plugin.initialized.load(Ordering::Relaxed) {
        return NOT_INITIALIZED;
    }
    let state = state != 0;
    // 既に同じ有効状態でも成功とする。終了時の重複した無効化も安全に扱う。
    plugin.active.store(state, Ordering::Relaxed);
    if !state {
        plugin.processing.store(false, Ordering::Relaxed);
        // SAFETY: setActive(false)はprocessの終了後に呼ばれる。
        unsafe { (*plugin.synth.get()).stop() };
    }
    OK
}

unsafe extern "system" fn empty_state(_: This, stream: This) -> TResult {
    // Controller固有の設定はなく、空の状態として成功する。streamは保持しない。
    if stream.is_null() {
        INVALID_ARGUMENT
    } else {
        OK
    }
}

unsafe extern "system" fn set_processor_state(this: This, stream: This) -> TResult {
    // SAFETY: ホストのストリームを検証し、成功時だけatomicへ反映する。
    match unsafe { parameters::read_state(stream) } {
        Ok(values) => {
            // SAFETY: thisは生存中のComponentインターフェース。
            let plugin = unsafe { instance(this) };
            plugin.processor_parameters.store(values);
            // ComponentとControllerは同一インスタンス。追加のsetComponentState呼び出しに
            // 依存せず表示も復元する。atomicの更新だけで、ホストへの編集通知は送らない。
            plugin.controller_parameters.store(values);
            OK
        }
        Err(status) => status,
    }
}
unsafe extern "system" fn get_processor_state(this: This, stream: This) -> TResult {
    // SAFETY: thisとstreamは生存中。DSPを借用せずatomicの値を保存する。
    unsafe { parameters::write_state(stream, instance(this).processor_parameters.snapshot()) }
}
unsafe extern "system" fn set_controller_component_state(this: This, stream: This) -> TResult {
    // SAFETY: ホストの状態を検証し、表示だけへ適用する。編集通知は送らない。
    match unsafe { parameters::read_state(stream) } {
        Ok(values) => {
            unsafe { instance(this) }
                .controller_parameters
                .store(values);
            OK
        }
        Err(status) => status,
    }
}
unsafe extern "system" fn parameter_count(_: This) -> i32 {
    (IDS.len() + 1) as i32
}
unsafe extern "system" fn parameter_info(_: This, index: i32, out: *mut ParameterInfo) -> TResult {
    if index == parameters::PROGRAM_INDEX as i32 && !out.is_null() {
        // SAFETY: outはホストのParameterInfo書き込み領域。
        unsafe {
            out.write(ParameterInfo {
                id: parameters::PROGRAM_ID,
                title: utf16("Program"),
                short_title: utf16("Program"),
                units: [0; 128],
                step_count: 0, // 1音色なのでprogramCount - 1 = 0。
                default_normalized_value: 0.0,
                unit_id: 0,
                flags: (1 << 3) | (1 << 15), // kIsList | kIsProgramChange
            });
        }
        return OK;
    }
    if !(0..IDS.len() as i32).contains(&index) || out.is_null() {
        return INVALID_ARGUMENT;
    }
    let i = index as usize;
    // SAFETY: outはホストの書き込み領域。
    unsafe {
        out.write(ParameterInfo {
            id: IDS[i],
            title: utf16(NAMES[i]),
            short_title: utf16(NAMES[i]),
            units: utf16(parameters::unit(i)),
            step_count: if parameters::choices(i).is_empty() {
                match i {
                    6 => 6,
                    8 => 31,
                    _ => 0,
                }
            } else {
                (parameters::choices(i).len() - 1) as i32
            },
            default_normalized_value: parameters::defaults()[i],
            unit_id: 0,
            flags: if !parameters::choices(i).is_empty() {
                1 | (1 << 3)
            } else {
                1
            },
        });
    }
    OK
}
unsafe extern "system" fn parameter_string(_: This, id: u32, value: f64, out: *mut u16) -> TResult {
    if id == parameters::PROGRAM_ID && (0.0..=1.0).contains(&value) {
        // SAFETY: outはnullまたはホストのString128。
        return unsafe { programs::program_name(ptr::null_mut(), 0, 0, out) };
    }
    let Some(i) = parameters::index(id) else {
        return INVALID_ARGUMENT;
    };
    if out.is_null() || !(0.0..=1.0).contains(&value) {
        return INVALID_ARGUMENT;
    }
    let names = parameters::choices(i);
    let text = if !names.is_empty() {
        names[parameters::to_plain(i, value) as usize].to_owned()
    } else if parameters::is_volume(i) && value == 0.0 {
        "−∞".to_owned()
    } else if matches!(i, 6 | 8) {
        format!("{:.0}", parameters::to_plain(i, value))
    } else {
        format!("{:.3}", parameters::to_plain(i, value))
    };
    let text: [u16; 128] = utf16(&text);
    // SAFETY: ホストのString128へコピーする。文字列操作はUIスレッドに限定。
    unsafe {
        ptr::copy_nonoverlapping(text.as_ptr(), out, text.len());
    }
    OK
}
unsafe extern "system" fn parameter_value(
    _: This,
    id: u32,
    text: *mut u16,
    out: *mut f64,
) -> TResult {
    if text.is_null() || out.is_null() {
        return INVALID_ARGUMENT;
    }
    let mut buffer = Vec::new();
    for offset in 0..128 {
        // SAFETY: NUL終端のホスト文字列。NULで停止し、最大128文字に制限する。
        let ch = unsafe { *text.add(offset) };
        if ch == 0 {
            break;
        }
        buffer.push(ch);
    }
    let Ok(text) = String::from_utf16(&buffer) else {
        return INVALID_ARGUMENT;
    };
    if id == parameters::PROGRAM_ID && text == parameters::PROGRAM_NAME {
        // SAFETY: outは検証済みの書き込み領域。
        unsafe { out.write(0.0) };
        return OK;
    }
    let Some(i) = parameters::index(id) else {
        return INVALID_ARGUMENT;
    };
    let names = parameters::choices(i);
    let plain = if !names.is_empty() {
        names
            .iter()
            .position(|&name| name == text)
            .map(|v| v as f64)
    } else if parameters::is_volume(i) && matches!(text.trim(), "−∞" | "-∞" | "-inf") {
        Some(f64::NEG_INFINITY)
    } else {
        text.trim().parse::<f64>().ok()
    };
    let Some(plain) = plain.filter(|&v| parameters::valid_plain(i, v)) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: outは正規化値の書き込み領域。
    unsafe {
        out.write(parameters::to_normalized(i, plain));
    }
    OK
}
unsafe extern "system" fn normalized_to_plain(_: This, id: u32, value: f64) -> f64 {
    if !(0.0..=1.0).contains(&value) {
        return 0.0;
    }
    parameters::index(id).map_or(0.0, |i| parameters::to_plain(i, value))
}
unsafe extern "system" fn plain_to_normalized(_: This, id: u32, value: f64) -> f64 {
    parameters::index(id)
        .filter(|&i| parameters::valid_plain(i, value))
        .map_or(0.0, |i| parameters::to_normalized(i, value))
}
unsafe extern "system" fn parameter_normalized(this: This, id: u32) -> f64 {
    // SAFETY: thisは生存中。Controllerのatomicだけを読む。
    parameters::index(id).map_or(0.0, |i| {
        unsafe { instance(this) }.controller_parameters.get(i)
    })
}
unsafe extern "system" fn set_parameter(this: This, id: u32, value: f64) -> TResult {
    if id == parameters::PROGRAM_ID && (0.0..=1.0).contains(&value) {
        // SAFETY: ホストUIスレッド上のController。DSPにはホストのProgramキューで配送する。
        let plugin = unsafe { instance(this) };
        plugin
            .controller_parameters
            .store(parameters::sine_preset());
        let handler = plugin.component_handler.borrow().clone();
        if let Some(handler) = handler {
            let p = handler.as_ptr();
            // SAFETY: 所有参照を保持し、再入するホスト呼び出し中はRefCellを借用しない。
            unsafe {
                let table = &**p.cast::<*const ComponentHandlerVTable>();
                (table.restart_component)(p, 1 << 2); // kParamValuesChanged
            }
        }
        return OK;
    }
    let Some(i) = parameters::index(id) else {
        return INVALID_ARGUMENT;
    };
    if !(0.0..=1.0).contains(&value) {
        return INVALID_ARGUMENT;
    }
    // SAFETY: thisは生存中。DSPへの配送はホストのVST3キューに任せる。
    unsafe { instance(this) }
        .controller_parameters
        .set(i, value);
    OK
}
unsafe extern "system" fn set_component_handler(this: This, handler: This) -> TResult {
    // SAFETY: UIスレッドの呼び出し。新しい所有参照を取ってから古い参照を解放する。
    let plugin = unsafe { instance(this) };
    let new_handler = unsafe { HostRef::retain(handler) };
    let old = plugin.component_handler.replace(new_handler);
    drop(old); // ホストへ戻る間RefCellの借用を保持しない。
    OK
}
unsafe extern "system" fn create_view(this: This, name: *const u8) -> This {
    // SAFETY: nameはnullまたはNUL終端文字列。標準のeditorだけを提供する。
    if name.is_null() || unsafe { std::ffi::CStr::from_ptr(name.cast()) }.to_bytes() != b"editor" {
        return ptr::null_mut();
    }
    // SAFETY: thisは生存中のController。Viewが独立した所有参照を取る。
    unsafe { crate::gui::create(this, Arc::clone(&instance(this).controller_parameters)) }
}

/// SAFETY: GUIの所有参照がthisを保持し、ホストのUIスレッドからのみ呼ぶ。
pub unsafe fn edit_parameter(this: This, index: usize, value: f64) -> TResult {
    if index >= IDS.len() || !(0.0..=1.0).contains(&value) {
        return INVALID_ARGUMENT;
    }
    let value = parameters::canonical(index, value);
    // SAFETY: GUIの所有参照がthisを保持。ホストコールバック前にRefCellの借用を終了する。
    let plugin = unsafe { instance(this) };
    if plugin.component_handler.borrow().is_none() {
        return FALSE;
    }
    // 編集配送前に再描画されると同じ値が複数回キューへ入るため、適用済みなら通知しない。
    if plugin.controller_parameters.get(index) == value {
        return OK;
    }
    // SAFETY: UIスレッドの生存中のcontrollerと、その所有するhandler。
    let Some(handler) = (unsafe { begin_parameter_edit(this, index) }) else {
        return FALSE;
    };
    let result = unsafe { perform_parameter_edit(this, &handler, index, value) };
    unsafe {
        end_parameter_edit(&handler, index);
    }
    result
}

/// SAFETY: 生存中のcontroller、範囲内のindexをホストUIスレッドから渡す。
pub(crate) unsafe fn begin_parameter_edit(this: This, index: usize) -> Option<HostRef> {
    let plugin = unsafe { instance(this) };
    let handler = plugin.component_handler.borrow().clone()?;
    let p = handler.as_ptr();
    // SAFETY: ローカル所有参照が再入によるhandler交換から保護する。
    let table = unsafe { &**p.cast::<*const ComponentHandlerVTable>() };
    (unsafe { (table.begin_edit)(p, IDS[index]) } == OK).then_some(handler)
}

/// SAFETY: UIスレッドから、beginEditが成功したhandlerと対応するindexを渡す。
pub(crate) unsafe fn perform_parameter_edit(
    this: This,
    handler: &HostRef,
    index: usize,
    value: f64,
) -> TResult {
    let value = parameters::canonical(index, value);
    let plugin = unsafe { instance(this) };
    if plugin.controller_parameters.get(index) == value {
        return OK;
    }
    plugin.controller_parameters.set(index, value);
    let p = handler.as_ptr();
    let table = unsafe { &**p.cast::<*const ComponentHandlerVTable>() };
    unsafe { (table.perform_edit)(p, IDS[index], value) }
}

/// SAFETY: UIスレッドから、開始済みの編集について一度だけ呼ぶ。
pub(crate) unsafe fn end_parameter_edit(handler: &HostRef, index: usize) {
    let p = handler.as_ptr();
    let table = unsafe { &**p.cast::<*const ComponentHandlerVTable>() };
    unsafe {
        (table.end_edit)(p, IDS[index]);
    }
}

unsafe extern "system" fn set_bus_arrangements(
    _: This,
    _: *mut u64,
    num_inputs: i32,
    outputs: *mut u64,
    num_outputs: i32,
) -> TResult {
    if num_inputs != 0 || num_outputs != 1 {
        return FALSE;
    }
    if outputs.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outputsはnum_outputs個のSpeakerArrangementを含む。
    if unsafe { *outputs } == STEREO {
        OK
    } else {
        FALSE
    }
}

unsafe extern "system" fn get_bus_arrangement(
    _: This,
    direction: i32,
    index: i32,
    out: *mut u64,
) -> TResult {
    if direction != OUTPUT || index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはSpeakerArrangementの書き込み領域。
    unsafe {
        out.write(STEREO);
    }
    OK
}

unsafe extern "system" fn can_process_sample_size(_: This, size: i32) -> TResult {
    if matches!(size, SAMPLE32 | SAMPLE64) {
        OK
    } else {
        FALSE
    }
}

unsafe extern "system" fn zero(_: This) -> u32 {
    0
}

unsafe extern "system" fn setup_processing(this: This, setup: *const ProcessSetup) -> TResult {
    if setup.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: setupは呼び出し中有効なProcessSetup。thisは生存中のInterface。
    let (plugin, setup) = unsafe { (instance(this), &*setup) };
    if !plugin.initialized.load(Ordering::Relaxed) {
        return NOT_INITIALIZED;
    }
    if plugin.active.load(Ordering::Relaxed) {
        return FALSE;
    }
    if !(0..=2).contains(&setup.process_mode)
        || !matches!(setup.symbolic_sample_size, SAMPLE32 | SAMPLE64)
        || setup.max_samples_per_block <= 0
        || !setup.sample_rate.is_finite()
        || setup.sample_rate < crate::filter::MIN_SAMPLE_RATE
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: setupProcessingは非アクティブ状態で呼ばれ、processと競合しない。
    unsafe { (*plugin.synth.get()).set_sample_rate(setup.sample_rate) };
    plugin
        .max_samples
        .store(setup.max_samples_per_block, Ordering::Relaxed);
    plugin
        .sample_size
        .store(setup.symbolic_sample_size, Ordering::Relaxed);
    OK
}

unsafe extern "system" fn get_tail_samples(_: This) -> u32 {
    // フィルタのIIR残響をホストに途中で打ち切らせない。kInfiniteTailを保守的に返す。
    u32::MAX
}

unsafe extern "system" fn set_processing(this: This, state: u8) -> TResult {
    // SAFETY: thisは生存中のInterface。確保・解放・ロックは行わない。
    let plugin = unsafe { instance(this) };
    if !plugin.active.load(Ordering::Relaxed) {
        return NOT_INITIALIZED;
    }
    plugin.processing.store(state != 0, Ordering::Relaxed);
    if state == 0 {
        // SAFETY: setProcessing(false)は全process呼び出しの終了後に呼ばれる。
        unsafe { (*plugin.synth.get()).stop() };
    }
    OK
}

unsafe extern "system" fn process(this: This, data: *mut ProcessData) -> TResult {
    if data.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: dataは呼び出し中有効なProcessData。入れ子のバッファはホスト所有。
    let (plugin, data) = unsafe { (instance(this), &*data) };
    if data.num_samples < 0
        || data.num_inputs != 0
        || !(0..=1).contains(&data.num_outputs)
        || !matches!(data.symbolic_sample_size, SAMPLE32 | SAMPLE64)
    {
        return INVALID_ARGUMENT;
    }
    // 停止中もゼロサンプルのパラメーターフラッシュは受理する。ノートは受け付けない。
    if !plugin.processing.load(Ordering::Relaxed) {
        if data.num_samples != 0 {
            return NOT_INITIALIZED;
        }
        // SAFETY: 変更キューは呼び出し中有効。停止中のDSP状態には触れない。
        let mut program = unsafe { ParameterPoints::new(data, parameters::PROGRAM_INDEX, 0.0) };
        let loaded = program.take_program_at(0);
        let mut values = if loaded {
            parameters::sine_preset()
        } else {
            plugin.processor_parameters.snapshot()
        };
        for (i, value) in values.iter_mut().enumerate() {
            // SAFETY: キューの借用期間はこのprocess内だけ。
            let mut changes = unsafe { ParameterPoints::new(data, i, *value) };
            *value = changes.value_at(0);
            if loaded || changes.automated {
                plugin.processor_parameters.set(i, *value);
            }
        }
        return OK;
    }
    if data.num_samples > plugin.max_samples.load(Ordering::Relaxed)
        || data.symbolic_sample_size != plugin.sample_size.load(Ordering::Relaxed)
    {
        return INVALID_ARGUMENT;
    }
    // 出力がなくてもノートと時間を処理する。ゼロサンプル時はバッファを参照しない。
    let channels = if data.num_samples > 0
        && data.num_outputs > 0
        && plugin.output_active.load(Ordering::Relaxed)
    {
        if data.outputs.is_null() {
            return INVALID_ARGUMENT;
        }
        // SAFETY: ホスト所有のバス記述子と2個のチャンネルポインター。
        let bus = unsafe { &*data.outputs };
        if bus.num_channels != 2 || bus.channel_buffers.is_null() {
            return INVALID_ARGUMENT;
        }
        let channels = unsafe { [*bus.channel_buffers, *bus.channel_buffers.add(1)] };
        if channels.iter().any(|channel| channel.is_null()) {
            return INVALID_ARGUMENT;
        }
        Some(channels)
    } else {
        None
    };

    // process内のイベント・パラメーター読出しを含む音声処理に限定する。
    // 戻り時にはホストのMXCSRを復元し、ブロック外へFTZ/DAZを持ち越さない。
    let _float_mode = crate::float_mode::FloatMode::flush_subnormals();
    // SAFETY: 同一インスタンスのprocess（ゼロサンプルも含む）およびDSPを更新する
    // ライフサイクル呼び出しをホストが直列化する。ホストからの再入processは許されない。
    // synthの参照・ホストのイベントやバッファのポインターを呼び出し後へ保持しない。
    let synth = unsafe { &mut *plugin.synth.get() };
    let values = plugin.processor_parameters.snapshot();
    synth.set_parameters(values);
    // SAFETY: 音色設定とProgramのカーソルはこのprocess中だけホストのキューを借用する。
    let mut changes: [ParameterPoints<'_>; parameters::COUNT + 1] =
        std::array::from_fn(|i| unsafe {
            ParameterPoints::new(data, i, values.get(i).copied().unwrap_or(0.0))
        });
    let event_active = plugin.event_active.load(Ordering::Relaxed);
    if !event_active {
        synth.stop();
    }
    let mut cursor = 0;
    let mut nonzero = 0;
    if event_active && !data.input_events.is_null() {
        // SAFETY: inputEventsはホスト所有のIEventList。vtableの先頭はFUnknown。
        let table = unsafe { &**data.input_events.cast::<*const EventListVTable>() };
        let count = unsafe { (table.get_event_count)(data.input_events) };
        for index in 0..count {
            let mut event = MaybeUninit::<Event>::zeroed();
            // SAFETY: Event全体の出力領域を渡す。成功時だけ対応するunion variantを読む。
            if unsafe { (table.get_event)(data.input_events, index, event.as_mut_ptr()) } != OK {
                continue;
            }
            let event = unsafe { event.assume_init() };
            if event.bus_index != 0
                || !matches!(event.event_type, 0 | 1)
                || event.sample_offset < cursor
                || (data.num_samples == 0 && event.sample_offset != 0)
                || (data.num_samples > 0 && event.sample_offset >= data.num_samples)
            {
                continue;
            }
            // SAFETY: channelsは検証済みで、区間は0..num_samples内。
            nonzero |= unsafe {
                generate_samples(
                    plugin,
                    synth,
                    data,
                    channels,
                    cursor..event.sample_offset,
                    &mut changes,
                )
            };
            cursor = event.sample_offset;
            if event.event_type == 0 {
                let note = unsafe { event.payload.note_on };
                synth.note_on(
                    Note {
                        id: note.note_id,
                        channel: note.channel,
                        pitch: note.pitch,
                    },
                    note.tuning,
                    note.velocity,
                );
            } else {
                let note = unsafe { event.payload.note_off };
                synth.note_off(Note {
                    id: note.note_id,
                    channel: note.channel,
                    pitch: note.pitch,
                });
            }
        }
    }
    // SAFETY: 最後のイベントからブロック末尾まで。バッファがなくても位相を進める。
    nonzero |= unsafe {
        generate_samples(
            plugin,
            synth,
            data,
            channels,
            cursor..data.num_samples,
            &mut changes,
        )
    };
    if channels.is_some() {
        // SAFETY: channelsがSomeのときは有効な出力バスが存在する。
        unsafe { (*data.outputs).silence_flags = STEREO & !nonzero };
    }
    OK
}

// SAFETY契約: channelsの各要素はnum_samples個の書き込み領域。rangeは範囲内。
// changesはこのprocessから借用し、
// これまでの区間と順に処理する。同時刻の変更をすべて適用してからノートを処理する。
unsafe fn generate_samples(
    plugin: &Plugin,
    synth: &mut Synthesizer,
    data: &ProcessData,
    channels: Option<[This; 2]>,
    range: std::ops::Range<i32>,
    changes: &mut [ParameterPoints<'_>; parameters::COUNT + 1],
) -> u64 {
    // バッファの有無と精度は区間ごとに選び、サンプルループ内では分岐しない。
    let nonzero = match channels {
        Some(channels) if data.symbolic_sample_size == SAMPLE32 => {
            generate_output_samples(plugin, synth, changes, range.clone(), |offset, sample| {
                let sample = sample.map(|value| value as f32);
                for (channel, sample) in channels.into_iter().zip(sample) {
                    // SAFETY: 呼び出し元で検証済み。左右が同じ領域でも順に書き込む。
                    unsafe { channel.cast::<f32>().add(offset).write(sample) };
                }
                u64::from(sample[0] != 0.0) | (u64::from(sample[1] != 0.0) << 1)
            })
        }
        Some(channels) => {
            generate_output_samples(plugin, synth, changes, range.clone(), |offset, sample| {
                for (channel, sample) in channels.into_iter().zip(sample) {
                    // SAFETY: 呼び出し元で検証済み。排他的sliceは作らない。
                    unsafe { channel.cast::<f64>().add(offset).write(sample) };
                }
                u64::from(sample[0] != 0.0) | (u64::from(sample[1] != 0.0) << 1)
            })
        }
        None => generate_output_samples(plugin, synth, changes, range.clone(), |_, _| 0),
    };
    // ノートと同位置のパラメーターを先に適用。0サンプルフラッシュもここを通る。
    if range.end < data.num_samples || data.num_samples == 0 {
        apply_parameters(plugin, synth, changes, range.end);
    }
    nonzero
}

/// 変更キューのない区間では、カーソル走査と設定比較をサンプルごとに行わない。
fn generate_output_samples(
    plugin: &Plugin,
    synth: &mut Synthesizer,
    changes: &mut [ParameterPoints<'_>; parameters::COUNT + 1],
    range: std::ops::Range<i32>,
    mut write: impl FnMut(usize, [f64; 2]) -> u64,
) -> u64 {
    let mut nonzero = 0;
    if changes.iter().any(|change| change.automated) {
        for offset in range {
            apply_parameters(plugin, synth, changes, offset);
            nonzero |= write(offset as usize, synth.generate_sample());
        }
    } else {
        for offset in range {
            nonzero |= write(offset as usize, synth.generate_sample());
        }
    }
    nonzero
}

fn apply_parameters(
    plugin: &Plugin,
    synth: &mut Synthesizer,
    changes: &mut [ParameterPoints<'_>; parameters::COUNT + 1],
    offset: i32,
) {
    let loaded = changes[parameters::PROGRAM_INDEX].take_program_at(offset);
    if loaded {
        let values = parameters::sine_preset();
        for (change, value) in changes.iter_mut().zip(values) {
            change.reset_value(offset, value);
        }
    }
    let values = std::array::from_fn(|i| changes[i].value_at(offset));
    let changed = synth.set_parameters(values);
    if loaded {
        plugin.processor_parameters.store(values);
    } else if changed {
        for (i, change) in changes[..parameters::COUNT].iter().enumerate() {
            if change.automated {
                plugin.processor_parameters.set(i, values[i]);
            }
        }
    }
}

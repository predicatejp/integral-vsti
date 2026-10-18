use crate::abi::*;
use std::ffi::c_void;
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering};

// vtableポインターを先頭に置く。ownerはホストに見えない内部データ。
#[repr(C)]
struct Interface {
    vtable: *const c_void,
    owner: *mut Plugin,
}

struct Plugin {
    component: Interface,
    processor: Interface,
    controller: Interface,
    context_requirements: Interface,
    refs: AtomicU32,
    initialized: AtomicBool,
    active: AtomicBool,
    processing: AtomicBool,
    output_active: AtomicBool,
    max_samples: AtomicI32,
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
    set_state: empty_state,
    get_state: empty_state,
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
    get_tail_samples: zero,
};
static CONTEXT_REQUIREMENTS: ContextRequirementsVTable = ContextRequirementsVTable {
    unknown: UNKNOWN,
    get_process_context_requirements: zero,
};
// ホストがパラメーターを照会できる最小Controller。GUIもパラメーターもない。
static CONTROLLER: ControllerVTable = ControllerVTable {
    unknown: UNKNOWN,
    initialize,
    terminate,
    set_component_state: empty_state,
    set_state: empty_state,
    get_state: empty_state,
    get_parameter_count: parameter_count,
    get_parameter_info: parameter_info,
    get_param_string_by_value: parameter_string,
    get_param_value_by_string: parameter_value,
    normalized_param_to_plain: parameter_conversion,
    plain_param_to_normalized: parameter_conversion,
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
        refs: AtomicU32::new(1),
        initialized: AtomicBool::new(false),
        active: AtomicBool::new(false),
        processing: AtomicBool::new(false),
        output_active: AtomicBool::new(false),
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
        _ => return ptr::null_mut(),
    };
    ptr::from_ref(target).cast_mut().cast()
}

// SAFETY契約: thisは公開済みInterfaceで、呼び出し元が所有参照を持つ。
// 生存中のownerだけを返し、可変共有状態はatomic経由で操作する。
unsafe fn instance<'a>(this: This) -> &'a Plugin {
    unsafe { &*(*this.cast::<Interface>()).owner }
}

unsafe extern "C" fn query(this: This, iid: *const Tuid, out: *mut This) -> TResult {
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

unsafe extern "C" fn add_ref(this: This) -> u32 {
    // SAFETY: thisの所有参照が呼び出しの間ownerを保持する。
    crate::retain(unsafe { &instance(this).refs })
}

unsafe extern "C" fn release(this: This) -> u32 {
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

unsafe extern "C" fn initialize(this: This, context: This) -> TResult {
    if context.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: thisは生存中のInterface。contextは利用・保持しない。
    if unsafe { instance(this) }
        .initialized
        .swap(true, Ordering::Relaxed)
    {
        FALSE
    } else {
        OK
    }
}

unsafe extern "C" fn terminate(this: This) -> TResult {
    // SAFETY: thisは生存中のInterface。終了はホストが処理停止後に呼ぶ。
    let plugin = unsafe { instance(this) };
    if !plugin.initialized.swap(false, Ordering::Relaxed) {
        return FALSE;
    }
    plugin.processing.store(false, Ordering::Relaxed);
    plugin.active.store(false, Ordering::Relaxed);
    plugin.output_active.store(false, Ordering::Relaxed);
    plugin.max_samples.store(0, Ordering::Relaxed);
    OK
}

unsafe extern "C" fn controller_id(_: This, out: *mut Tuid) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outは16バイトの書き込み領域。Controllerは同一オブジェクトのqueryで提供する。
    unsafe {
        out.write([0; 16]);
    }
    FALSE
}

unsafe extern "C" fn set_io_mode(_: This, mode: i32) -> TResult {
    if (0..=2).contains(&mode) {
        OK
    } else {
        INVALID_ARGUMENT
    }
}

fn has_bus(media: i32, direction: i32) -> bool {
    matches!((media, direction), (AUDIO, OUTPUT) | (EVENT, INPUT))
}

unsafe extern "C" fn bus_count(_: This, media: i32, direction: i32) -> i32 {
    i32::from(has_bus(media, direction))
}

unsafe extern "C" fn bus_info(
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

unsafe extern "C" fn routing_info(_: This, _: *mut RoutingInfo, _: *mut RoutingInfo) -> TResult {
    NOT_IMPLEMENTED
}

unsafe extern "C" fn activate_bus(
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
        // SAFETY: thisは生存中のInterface。イベントは有効状態にかかわらず無視する。
        unsafe { instance(this) }
            .output_active
            .store(state != 0, Ordering::Relaxed);
    }
    OK
}

unsafe extern "C" fn set_active(this: This, state: u8) -> TResult {
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
    }
    OK
}

unsafe extern "C" fn empty_state(_: This, stream: This) -> TResult {
    // 保存すべき設定がないため空の状態として成功する。streamは保持しない。
    if stream.is_null() {
        INVALID_ARGUMENT
    } else {
        OK
    }
}

unsafe extern "C" fn parameter_count(_: This) -> i32 {
    0
}
unsafe extern "C" fn parameter_info(_: This, _: i32, _: *mut c_void) -> TResult {
    INVALID_ARGUMENT
}
unsafe extern "C" fn parameter_string(_: This, _: u32, _: f64, _: *mut u16) -> TResult {
    INVALID_ARGUMENT
}
unsafe extern "C" fn parameter_value(_: This, _: u32, _: *mut u16, _: *mut f64) -> TResult {
    INVALID_ARGUMENT
}
unsafe extern "C" fn parameter_conversion(_: This, _: u32, _: f64) -> f64 {
    0.0
}
unsafe extern "C" fn parameter_normalized(_: This, _: u32) -> f64 {
    0.0
}
unsafe extern "C" fn set_parameter(_: This, _: u32, _: f64) -> TResult {
    INVALID_ARGUMENT
}
unsafe extern "C" fn set_component_handler(_: This, _: This) -> TResult {
    // 通知を発生させないのでhandlerを保持せず、その参照カウントも変更しない。
    OK
}
unsafe extern "C" fn create_view(_: This, _: *const u8) -> This {
    ptr::null_mut()
}

unsafe extern "C" fn set_bus_arrangements(
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

unsafe extern "C" fn get_bus_arrangement(
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

unsafe extern "C" fn can_process_sample_size(_: This, size: i32) -> TResult {
    if matches!(size, SAMPLE32 | SAMPLE64) {
        OK
    } else {
        FALSE
    }
}

unsafe extern "C" fn zero(_: This) -> u32 {
    0
}

unsafe extern "C" fn setup_processing(this: This, setup: *const ProcessSetup) -> TResult {
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
        || setup.sample_rate <= 0.0
    {
        return INVALID_ARGUMENT;
    }
    plugin
        .max_samples
        .store(setup.max_samples_per_block, Ordering::Relaxed);
    plugin
        .sample_size
        .store(setup.symbolic_sample_size, Ordering::Relaxed);
    OK
}

unsafe extern "C" fn set_processing(this: This, state: u8) -> TResult {
    // SAFETY: thisは生存中のInterface。確保・解放・ロックは行わない。
    let plugin = unsafe { instance(this) };
    if !plugin.active.load(Ordering::Relaxed) {
        return NOT_INITIALIZED;
    }
    plugin.processing.store(state != 0, Ordering::Relaxed);
    OK
}

unsafe extern "C" fn process(this: This, data: *mut ProcessData) -> TResult {
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
    // 音声処理停止中にも到着するフラッシュ。処理するパラメーターはない。
    if data.num_samples == 0 {
        return OK;
    }
    if !plugin.processing.load(Ordering::Relaxed) {
        return NOT_INITIALIZED;
    }
    if data.num_samples > plugin.max_samples.load(Ordering::Relaxed)
        || data.symbolic_sample_size != plugin.sample_size.load(Ordering::Relaxed)
    {
        return INVALID_ARGUMENT;
    }
    if data.num_outputs == 0 {
        return OK;
    }
    if data.outputs.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストは出力バス記述子を提供し、呼び出し中の書き込みを許可する。
    let bus = unsafe { &mut *data.outputs };
    if !plugin.output_active.load(Ordering::Relaxed) {
        return OK;
    }
    if bus.num_channels != 2 || bus.channel_buffers.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストは2チャンネルそれぞれにnum_samples個の書き込み領域を提供する。
    // チャンネルが同じ領域を指していてもよいので、排他的sliceを作らず順にゼロ化する。
    unsafe {
        let left = *bus.channel_buffers;
        let right = *bus.channel_buffers.add(1);
        if left.is_null() || right.is_null() {
            return INVALID_ARGUMENT;
        }
        for channel in [left, right] {
            if data.symbolic_sample_size == SAMPLE32 {
                ptr::write_bytes(channel.cast::<f32>(), 0, data.num_samples as usize);
            } else {
                ptr::write_bytes(channel.cast::<f64>(), 0, data.num_samples as usize);
            }
        }
    }
    bus.silence_flags = STEREO;
    OK
}

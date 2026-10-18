//! Linux x86_64向けのVST3 ABI定義。必要なインターフェースだけを直接定義する。
//! 型・順序・IDは公式VST3インターフェース仕様と対応させる。

use std::ffi::c_void;

pub type Tuid = [u8; 16];
pub type TResult = i32;
pub type This = *mut c_void;

// LinuxではCOM互換のHRESULTではなく、この値を使用する。
pub const OK: TResult = 0;
pub const FALSE: TResult = 1;
pub const INVALID_ARGUMENT: TResult = 2;
pub const NOT_IMPLEMENTED: TResult = 3;
pub const NOT_INITIALIZED: TResult = 5;
pub const NO_INTERFACE: TResult = -1;

pub const UNKNOWN_IID: Tuid = 0x00000000_00000000_c0000000_00000046_u128.to_be_bytes();
pub const PLUGIN_BASE_IID: Tuid = 0x22888ddb_156e45ae_8358b348_08190625_u128.to_be_bytes();
pub const FACTORY_IID: Tuid = 0x7a4d811c_52114a1f_aed9d2ee_0b43bf9f_u128.to_be_bytes();
pub const FACTORY2_IID: Tuid = 0x0007b650_f24b4c0b_a464edb9_f00b2abb_u128.to_be_bytes();
pub const COMPONENT_IID: Tuid = 0xe831ff31_f2d54301_928ebbee_25697802_u128.to_be_bytes();
pub const PROCESSOR_IID: Tuid = 0x42043f99_b7da453c_a569e79d_9aaec33d_u128.to_be_bytes();
pub const CONTROLLER_IID: Tuid = 0xdcd7bbe3_7742448d_a874aacc_979c759e_u128.to_be_bytes();
pub const CONTEXT_REQUIREMENTS_IID: Tuid = 0x2a654303_ef764e3d_95b5fe83_730ef6d0_u128.to_be_bytes();

// 製品固有のクラスID。通常の更新で変更しない。
pub const COMPONENT_CID: Tuid = 0x9ad1ae8c_b51b4aac_a13c85c0_b023ab4d_u128.to_be_bytes();

pub const AUDIO: i32 = 0;
pub const EVENT: i32 = 1;
pub const INPUT: i32 = 0;
pub const OUTPUT: i32 = 1;
pub const SAMPLE32: i32 = 0;
pub const SAMPLE64: i32 = 1;
pub const STEREO: u64 = 3;

#[repr(C)]
pub struct UnknownVTable {
    pub query_interface: unsafe extern "C" fn(This, *const Tuid, *mut This) -> TResult,
    pub add_ref: unsafe extern "C" fn(This) -> u32,
    pub release: unsafe extern "C" fn(This) -> u32,
}

#[repr(C)]
pub struct FactoryVTable {
    pub unknown: UnknownVTable,
    pub get_factory_info: unsafe extern "C" fn(This, *mut FactoryInfo) -> TResult,
    pub count_classes: unsafe extern "C" fn(This) -> i32,
    pub get_class_info: unsafe extern "C" fn(This, i32, *mut ClassInfo) -> TResult,
    pub create_instance: unsafe extern "C" fn(This, *const Tuid, *const Tuid, *mut This) -> TResult,
    pub get_class_info2: unsafe extern "C" fn(This, i32, *mut ClassInfo2) -> TResult,
}

#[repr(C)]
pub struct ComponentVTable {
    pub unknown: UnknownVTable,
    pub initialize: unsafe extern "C" fn(This, This) -> TResult,
    pub terminate: unsafe extern "C" fn(This) -> TResult,
    pub get_controller_class_id: unsafe extern "C" fn(This, *mut Tuid) -> TResult,
    pub set_io_mode: unsafe extern "C" fn(This, i32) -> TResult,
    pub get_bus_count: unsafe extern "C" fn(This, i32, i32) -> i32,
    pub get_bus_info: unsafe extern "C" fn(This, i32, i32, i32, *mut BusInfo) -> TResult,
    pub get_routing_info: unsafe extern "C" fn(This, *mut RoutingInfo, *mut RoutingInfo) -> TResult,
    pub activate_bus: unsafe extern "C" fn(This, i32, i32, i32, u8) -> TResult,
    pub set_active: unsafe extern "C" fn(This, u8) -> TResult,
    pub set_state: unsafe extern "C" fn(This, This) -> TResult,
    pub get_state: unsafe extern "C" fn(This, This) -> TResult,
}

#[repr(C)]
pub struct ProcessorVTable {
    pub unknown: UnknownVTable,
    pub set_bus_arrangements: unsafe extern "C" fn(This, *mut u64, i32, *mut u64, i32) -> TResult,
    pub get_bus_arrangement: unsafe extern "C" fn(This, i32, i32, *mut u64) -> TResult,
    pub can_process_sample_size: unsafe extern "C" fn(This, i32) -> TResult,
    pub get_latency_samples: unsafe extern "C" fn(This) -> u32,
    pub setup_processing: unsafe extern "C" fn(This, *const ProcessSetup) -> TResult,
    pub set_processing: unsafe extern "C" fn(This, u8) -> TResult,
    pub process: unsafe extern "C" fn(This, *mut ProcessData) -> TResult,
    pub get_tail_samples: unsafe extern "C" fn(This) -> u32,
}

#[repr(C)]
pub struct ContextRequirementsVTable {
    pub unknown: UnknownVTable,
    pub get_process_context_requirements: unsafe extern "C" fn(This) -> u32,
}

#[repr(C)]
pub struct ControllerVTable {
    pub unknown: UnknownVTable,
    pub initialize: unsafe extern "C" fn(This, This) -> TResult,
    pub terminate: unsafe extern "C" fn(This) -> TResult,
    pub set_component_state: unsafe extern "C" fn(This, This) -> TResult,
    pub set_state: unsafe extern "C" fn(This, This) -> TResult,
    pub get_state: unsafe extern "C" fn(This, This) -> TResult,
    pub get_parameter_count: unsafe extern "C" fn(This) -> i32,
    // パラメーターは0個なのでParameterInfoの内容にはアクセスしない。
    pub get_parameter_info: unsafe extern "C" fn(This, i32, *mut c_void) -> TResult,
    pub get_param_string_by_value: unsafe extern "C" fn(This, u32, f64, *mut u16) -> TResult,
    pub get_param_value_by_string: unsafe extern "C" fn(This, u32, *mut u16, *mut f64) -> TResult,
    pub normalized_param_to_plain: unsafe extern "C" fn(This, u32, f64) -> f64,
    pub plain_param_to_normalized: unsafe extern "C" fn(This, u32, f64) -> f64,
    pub get_param_normalized: unsafe extern "C" fn(This, u32) -> f64,
    pub set_param_normalized: unsafe extern "C" fn(This, u32, f64) -> TResult,
    pub set_component_handler: unsafe extern "C" fn(This, This) -> TResult,
    pub create_view: unsafe extern "C" fn(This, *const u8) -> This,
}

#[repr(C)]
pub struct FactoryInfo {
    pub vendor: [u8; 64],
    pub url: [u8; 256],
    pub email: [u8; 128],
    pub flags: i32,
}

#[repr(C)]
pub struct ClassInfo {
    pub cid: Tuid,
    pub cardinality: i32,
    pub category: [u8; 32],
    pub name: [u8; 64],
}

#[repr(C)]
pub struct ClassInfo2 {
    pub base: ClassInfo,
    pub class_flags: u32,
    pub sub_categories: [u8; 128],
    pub vendor: [u8; 64],
    pub version: [u8; 64],
    pub sdk_version: [u8; 64],
}

#[repr(C)]
pub struct BusInfo {
    pub media_type: i32,
    pub direction: i32,
    pub channel_count: i32,
    pub name: [u16; 128],
    pub bus_type: i32,
    pub flags: u32,
}

#[repr(C)]
pub struct RoutingInfo {
    pub media_type: i32,
    pub bus_index: i32,
    pub channel: i32,
}

#[repr(C)]
pub struct ProcessSetup {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub max_samples_per_block: i32,
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    pub num_channels: i32,
    pub silence_flags: u64,
    // SDKのfloat** / double** unionと同じサイズ・配置。形式に応じてキャストする。
    pub channel_buffers: *mut *mut c_void,
}

#[repr(C)]
pub struct ProcessData {
    pub process_mode: i32,
    pub symbolic_sample_size: i32,
    pub num_samples: i32,
    pub num_inputs: i32,
    pub num_outputs: i32,
    pub inputs: *mut AudioBusBuffers,
    pub outputs: *mut AudioBusBuffers,
    pub input_parameter_changes: This,
    pub output_parameter_changes: This,
    pub input_events: This,
    pub output_events: This,
    pub process_context: This,
}

pub fn ascii<const N: usize>(text: &str) -> [u8; N] {
    let mut result = [0; N];
    for (dst, src) in result
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.bytes())
    {
        *dst = src;
    }
    result
}

pub fn utf16<const N: usize>(text: &str) -> [u16; N] {
    let mut result = [0; N];
    for (dst, src) in result
        .iter_mut()
        .take(N.saturating_sub(1))
        .zip(text.encode_utf16())
    {
        *dst = src;
    }
    result
}

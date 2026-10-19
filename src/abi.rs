// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only AND MIT

//! Linux/Windows x86_64向けのVST3 ABI定義。必要なインターフェースだけを直接定義する。
//! 型・順序・IDは公式VST3インターフェース仕様と対応させる。
//! 参照・移植元、固定リビジョン、MITライセンス全文はルートのTHIRD_PARTY_NOTICES.txtを参照。

use std::ffi::c_void;

pub type Tuid = [u8; 16];
pub type TResult = i32;
pub type This = *mut c_void;

pub const OK: TResult = 0;
pub const FALSE: TResult = 1;
#[cfg(not(target_os = "windows"))]
mod results {
    use super::TResult;
    pub const INVALID_ARGUMENT: TResult = 2;
    pub const NOT_IMPLEMENTED: TResult = 3;
    pub const NOT_INITIALIZED: TResult = 5;
    pub const NO_INTERFACE: TResult = -1;
}
#[cfg(target_os = "windows")]
mod results {
    use super::TResult;
    pub const INVALID_ARGUMENT: TResult = 0x80070057u32 as TResult;
    pub const NOT_IMPLEMENTED: TResult = 0x80004001u32 as TResult;
    pub const NOT_INITIALIZED: TResult = 0x8000ffffu32 as TResult;
    pub const NO_INTERFACE: TResult = 0x80004002u32 as TResult;
}
pub use results::*;

// SDKのINLINE_UIDと同じ表現。WindowsではGUIDの先頭4/2/2バイトを反転する。
const fn tuid(value: u128) -> Tuid {
    let b = value.to_be_bytes();
    if cfg!(target_os = "windows") {
        [
            b[3], b[2], b[1], b[0], b[5], b[4], b[7], b[6], b[8], b[9], b[10], b[11], b[12], b[13],
            b[14], b[15],
        ]
    } else {
        b
    }
}

pub const UNKNOWN_IID: Tuid = tuid(0x00000000_00000000_c0000000_00000046);
pub const PLUGIN_BASE_IID: Tuid = tuid(0x22888ddb_156e45ae_8358b348_08190625);
pub const FACTORY_IID: Tuid = tuid(0x7a4d811c_52114a1f_aed9d2ee_0b43bf9f);
pub const FACTORY2_IID: Tuid = tuid(0x0007b650_f24b4c0b_a464edb9_f00b2abb);
pub const COMPONENT_IID: Tuid = tuid(0xe831ff31_f2d54301_928ebbee_25697802);
pub const PROCESSOR_IID: Tuid = tuid(0x42043f99_b7da453c_a569e79d_9aaec33d);
pub const CONTROLLER_IID: Tuid = tuid(0xdcd7bbe3_7742448d_a874aacc_979c759e);
pub const CONTEXT_REQUIREMENTS_IID: Tuid = tuid(0x2a654303_ef764e3d_95b5fe83_730ef6d0);

// 製品固有のクラスID。通常の更新で変更しない。
pub const COMPONENT_CID: Tuid = tuid(0x9ad1ae8c_b51b4aac_a13c85c0_b023ab4d);

pub const AUDIO: i32 = 0;
pub const EVENT: i32 = 1;
pub const INPUT: i32 = 0;
pub const OUTPUT: i32 = 1;
pub const SAMPLE32: i32 = 0;
pub const SAMPLE64: i32 = 1;
pub const STEREO: u64 = 3;

#[repr(C)]
pub struct UnknownVTable {
    /// 指定IIDのインターフェースを取得し、成功時は所有参照を1つ返す。
    pub query_interface: unsafe extern "system" fn(This, *const Tuid, *mut This) -> TResult,
    /// 所有参照を1つ追加し、更新後の参照数を返す。
    pub add_ref: unsafe extern "system" fn(This) -> u32,
    /// 所有参照を1つ解放し、更新後の参照数を返す。最後の参照で実体を破棄する。
    pub release: unsafe extern "system" fn(This) -> u32,
}

#[repr(C)]
pub struct FactoryVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// 提供者の名称・連絡先・ファクトリーの属性を出力する。
    pub get_factory_info: unsafe extern "system" fn(This, *mut FactoryInfo) -> TResult,
    /// 生成可能なプラグインクラスの数を返す。
    pub count_classes: unsafe extern "system" fn(This) -> i32,
    /// 列挙位置に対応するクラスの基本情報を出力する。
    pub get_class_info: unsafe extern "system" fn(This, i32, *mut ClassInfo) -> TResult,
    /// クラスIDとIIDを指定してインスタンスを生成し、所有参照を返す。
    pub create_instance:
        unsafe extern "system" fn(This, *const Tuid, *const Tuid, *mut This) -> TResult,
    /// 列挙位置に対応するクラスの拡張情報を出力する。
    pub get_class_info2: unsafe extern "system" fn(This, i32, *mut ClassInfo2) -> TResult,
}

#[repr(C)]
pub struct ComponentVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// ホストのコンテキストを受け取り、インスタンスを初期化する。
    pub initialize: unsafe extern "system" fn(This, This) -> TResult,
    /// 処理停止後にインスタンスの初期化状態を終了する。
    pub terminate: unsafe extern "system" fn(This) -> TResult,
    /// 独立したControllerのクラスIDを問い合わせる。本実装は同一実体で提供する。
    pub get_controller_class_id: unsafe extern "system" fn(This, *mut Tuid) -> TResult,
    /// ホストが使用する入出力モードを通知する。
    pub set_io_mode: unsafe extern "system" fn(This, i32) -> TResult,
    /// メディア種別と入出力方向に対応するバス数を返す。
    pub get_bus_count: unsafe extern "system" fn(This, i32, i32) -> i32,
    /// 指定したバスのチャンネル数・名称・属性を出力する。
    pub get_bus_info: unsafe extern "system" fn(This, i32, i32, i32, *mut BusInfo) -> TResult,
    /// 入力から出力へのルーティングを問い合わせる。本実装では未対応。
    pub get_routing_info:
        unsafe extern "system" fn(This, *mut RoutingInfo, *mut RoutingInfo) -> TResult,
    /// 指定バスの有効・無効を切り替える。
    pub activate_bus: unsafe extern "system" fn(This, i32, i32, i32, u8) -> TResult,
    /// 音源インスタンスを有効化・無効化する。
    pub set_active: unsafe extern "system" fn(This, u8) -> TResult,
    /// ホストのIBStreamから音源の設定を復元する。
    pub set_state: unsafe extern "system" fn(This, This) -> TResult,
    /// ホストのIBStreamへ音源の設定を保存する。
    pub get_state: unsafe extern "system" fn(This, This) -> TResult,
}

#[repr(C)]
pub struct ProcessorVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// 入出力バスのスピーカー構成を設定する。
    pub set_bus_arrangements:
        unsafe extern "system" fn(This, *mut u64, i32, *mut u64, i32) -> TResult,
    /// 指定した音声バスのスピーカー構成を出力する。
    pub get_bus_arrangement: unsafe extern "system" fn(This, i32, i32, *mut u64) -> TResult,
    /// 指定されたfloat32／float64の処理に対応するかを返す。
    pub can_process_sample_size: unsafe extern "system" fn(This, i32) -> TResult,
    /// 処理による遅延をサンプル数で返す。本実装は0。
    pub get_latency_samples: unsafe extern "system" fn(This) -> u32,
    /// 非アクティブ状態で処理モード・精度・最大ブロック長・サンプルレートを設定する。
    pub setup_processing: unsafe extern "system" fn(This, *const ProcessSetup) -> TResult,
    /// processの呼び出し開始・終了を通知する。終了時は消音する。
    pub set_processing: unsafe extern "system" fn(This, u8) -> TResult,
    /// 1ブロックのイベントを処理し、ホストの音声バッファへ出力する。
    pub process: unsafe extern "system" fn(This, *mut ProcessData) -> TResult,
    /// 入力停止後の残響長をサンプル数で返す。本実装は0。
    pub get_tail_samples: unsafe extern "system" fn(This) -> u32,
}

#[repr(C)]
pub struct ContextRequirementsVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// 必要とするProcessContext情報のビットマスク。本実装は要求なしの0を返す。
    pub get_process_context_requirements: unsafe extern "system" fn(This) -> u32,
}

#[repr(C)]
pub struct ControllerVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// ホストのコンテキストを受け取り、インスタンスを初期化する。
    pub initialize: unsafe extern "system" fn(This, This) -> TResult,
    /// 処理停止後にインスタンスの初期化状態を終了する。
    pub terminate: unsafe extern "system" fn(This) -> TResult,
    /// 音源側の保存状態をControllerへ反映する。
    pub set_component_state: unsafe extern "system" fn(This, This) -> TResult,
    /// ホストのストリームから状態を復元する。本実装の保存状態は空。
    pub set_state: unsafe extern "system" fn(This, This) -> TResult,
    /// ホストのストリームへ状態を保存する。本実装の保存状態は空。
    pub get_state: unsafe extern "system" fn(This, This) -> TResult,
    /// ホストへ公開するパラメーター数を返す。
    pub get_parameter_count: unsafe extern "system" fn(This) -> i32,
    /// 列挙位置に対応するパラメーター情報を出力する。
    pub get_parameter_info: unsafe extern "system" fn(This, i32, *mut ParameterInfo) -> TResult,
    /// ParamIDと正規化値から表示文字列へ変換する。
    pub get_param_string_by_value: unsafe extern "system" fn(This, u32, f64, *mut u16) -> TResult,
    /// ParamIDと表示文字列から正規化値へ変換する。
    pub get_param_value_by_string:
        unsafe extern "system" fn(This, u32, *mut u16, *mut f64) -> TResult,
    /// ParamIDで指定した値を正規化値から実単位へ変換する。
    pub normalized_param_to_plain: unsafe extern "system" fn(This, u32, f64) -> f64,
    /// ParamIDで指定した値を実単位から正規化値へ変換する。
    pub plain_param_to_normalized: unsafe extern "system" fn(This, u32, f64) -> f64,
    /// ParamIDに対応する現在の正規化値を返す。
    pub get_param_normalized: unsafe extern "system" fn(This, u32) -> f64,
    /// ParamIDに対応する正規化値を更新する。
    pub set_param_normalized: unsafe extern "system" fn(This, u32, f64) -> TResult,
    /// ホストへの変更通知用handlerを設定する。本実装では保持しない。
    pub set_component_handler: unsafe extern "system" fn(This, This) -> TResult,
    /// 指定種類のGUIを生成する。本実装はGUIを持たずnullを返す。
    pub create_view: unsafe extern "system" fn(This, *const u8) -> This,
}

#[repr(C)]
pub struct FactoryInfo {
    /// NUL終端の提供者名。
    pub vendor: [u8; 64],
    /// NUL終端の提供者URL。
    pub url: [u8; 256],
    /// NUL終端の問い合わせ先メールアドレス。
    pub email: [u8; 128],
    /// ファクトリーの対応機能を表すビットマスク。
    pub flags: i32,
}

#[repr(C)]
pub struct ClassInfo {
    /// プラグインクラスを識別する16バイトのID。
    pub cid: Tuid,
    /// このクラスで生成できるインスタンス数の上限。
    pub cardinality: i32,
    /// NUL終端のクラス分類。本実装はAudio Module Class。
    pub category: [u8; 32],
    /// NUL終端のホスト表示用クラス名。
    pub name: [u8; 64],
}

#[repr(C)]
pub struct ClassInfo2 {
    /// 拡張情報の先頭を占める基本クラス情報。
    pub base: ClassInfo,
    /// クラスの機能・動作属性を表すビットマスク。
    pub class_flags: u32,
    /// NUL終端の詳細分類。本実装はInstrument|Synth。
    pub sub_categories: [u8; 128],
    /// NUL終端の提供者名。
    pub vendor: [u8; 64],
    /// NUL終端の音源バージョン。正式版はSemVer、開発版はdev-コミットID。
    pub version: [u8; 64],
    /// NUL終端の準拠SDKバージョン。
    pub sdk_version: [u8; 64],
}

#[repr(C)]
pub struct BusInfo {
    /// メディア種別。0は音声、1はイベント。
    pub media_type: i32,
    /// 入出力方向。0は入力、1は出力。
    pub direction: i32,
    /// このバスのチャンネル数。本実装では音声出力2、イベント入力16。
    pub channel_count: i32,
    /// NUL終端のUTF-16バス名。
    pub name: [u16; 128],
    /// バスの用途。0はメイン、1は補助。
    pub bus_type: i32,
    /// バスの属性ビット。本実装の1はデフォルトでの有効化を推奨する。
    pub flags: u32,
}

#[repr(C)]
pub struct ParameterInfo {
    /// 更新後も変更しないパラメーター識別子。列挙位置とは別。
    pub id: u32,
    /// NUL終端のUTF-16表示名。
    pub title: [u16; 128],
    /// NUL終端のUTF-16短縮名。
    pub short_title: [u16; 128],
    /// NUL終端のUTF-16単位。波形選択には単位なし。
    pub units: [u16; 128],
    /// 離散値の最大値と最小値の差。4択なら3。
    pub step_count: i32,
    /// 0..=1で表す既定値。
    pub default_normalized_value: f64,
    /// 所属ユニット。0はルート。
    pub unit_id: i32,
    /// kCanAutomate、kIsList等の属性ビット。
    pub flags: i32,
}

#[repr(C)]
pub struct ParameterChangesVTable {
    /// FUnknownの共通メソッド。
    pub unknown: UnknownVTable,
    /// 変更されたパラメーターのキュー数。
    pub get_parameter_count: unsafe extern "system" fn(This) -> i32,
    /// 列挙位置に対応する借用IParamValueQueue。null可。
    pub get_parameter_data: unsafe extern "system" fn(This, i32) -> This,
    /// ParamIDのキューを追加する。本実装では呼び出さない。
    pub add_parameter_data: unsafe extern "system" fn(This, *const u32, *mut i32) -> This,
}

#[repr(C)]
pub struct ParamValueQueueVTable {
    /// FUnknownの共通メソッド。
    pub unknown: UnknownVTable,
    /// キューが保持するパラメーターの安定した識別子。
    pub get_parameter_id: unsafe extern "system" fn(This) -> u32,
    /// キュー内の変更点数。
    pub get_point_count: unsafe extern "system" fn(This) -> i32,
    /// 指定位置のsampleOffsetと正規化値を出力する。成功時だけ出力を使用する。
    pub get_point: unsafe extern "system" fn(This, i32, *mut i32, *mut f64) -> TResult,
    /// 変更点を追加する。本実装では呼び出さない。
    pub add_point: unsafe extern "system" fn(This, i32, f64, *mut i32) -> TResult,
}

#[repr(C)]
pub struct StreamVTable {
    /// FUnknownの共通メソッド。
    pub unknown: UnknownVTable,
    /// 最大numBytesバイトを読み、実際の読み取り長を出力する。
    pub read: unsafe extern "system" fn(This, This, i32, *mut i32) -> TResult,
    /// 最大numBytesバイトを書き、実際の書き込み長を出力する。
    pub write: unsafe extern "system" fn(This, This, i32, *mut i32) -> TResult,
    /// 基準位置（先頭・現在・末尾）から移動し、移動後の位置を出力する。
    pub seek: unsafe extern "system" fn(This, i64, i32, *mut i64) -> TResult,
    /// 現在の読み書き位置を出力する。
    pub tell: unsafe extern "system" fn(This, *mut i64) -> TResult,
}

#[repr(C)]
pub struct RoutingInfo {
    /// メディア種別。0は音声、1はイベント。
    pub media_type: i32,
    /// メディア種別・方向ごとの0始まりのバス番号。
    pub bus_index: i32,
    /// バス内の0始まりのチャンネル番号。
    pub channel: i32,
}

#[repr(C)]
pub struct ProcessSetup {
    /// 処理モード。0はリアルタイム、1はプリフェッチ、2はオフライン。
    pub process_mode: i32,
    /// 音声サンプルの形式。SAMPLE32はf32、SAMPLE64はf64。
    pub symbolic_sample_size: i32,
    /// 1回のprocessで処理できる最大サンプル数。実際の処理長とは別。
    pub max_samples_per_block: i32,
    /// 1秒あたりのサンプル数（Hz）。有限の正数。
    pub sample_rate: f64,
}

#[repr(C)]
pub struct AudioBusBuffers {
    /// この音声バスが持つチャンネルバッファの数。
    pub num_channels: i32,
    /// チャンネルごとの無音ビット。ビットnが1ならチャンネルnの全サンプルが無音。
    pub silence_flags: u64,
    /// ホスト所有のチャンネル先頭ポインター配列。各領域はnum_samples個のサンプルを持つ。
    /// SDKのfloat** / double** unionと同じサイズ・配置。形式に応じてキャストする。
    pub channel_buffers: *mut *mut c_void,
}

#[repr(C)]
pub struct ProcessData {
    /// 処理モード。0はリアルタイム、1はプリフェッチ、2はオフライン。
    pub process_mode: i32,
    /// 音声サンプルの形式。SAMPLE32はf32、SAMPLE64はf64。
    pub symbolic_sample_size: i32,
    /// 今回のブロックのサンプル数。0は音声生成を伴わないフラッシュ。
    pub num_samples: i32,
    /// inputsが指す音声入力バスの数。本実装は0。
    pub num_inputs: i32,
    /// outputsが指す音声出力バスの数。本実装は0または1を受け付ける。
    pub num_outputs: i32,
    /// ホスト所有の音声入力バス配列。本実装では参照しない。
    pub inputs: *mut AudioBusBuffers,
    /// ホスト所有の音声出力バス配列。処理中だけ借用する。
    pub outputs: *mut AudioBusBuffers,
    /// ホストからのIParameterChanges。null可。process中だけ借用する。
    pub input_parameter_changes: This,
    /// ホストへ返すIParameterChanges。null可。本実装では書き込まない。
    pub output_parameter_changes: This,
    /// ホストからのIEventList。nullはイベントなし。処理中だけ借用する。
    pub input_events: This,
    /// ホストへ返すIEventList。null可。本実装ではイベントを出力しない。
    pub output_events: This,
    /// ホストの再生位置・テンポ等のProcessContext。null可。本実装では参照しない。
    pub process_context: This,
}

#[repr(C)]
pub struct EventListVTable {
    /// FUnknownの共通メソッド。派生インターフェースでもvtableの先頭に配置する。
    pub unknown: UnknownVTable,
    /// リスト内のイベント数を返す。
    pub get_event_count: unsafe extern "system" fn(This) -> i32,
    /// 0始まりの位置でイベントを取得する。成功時だけ出力領域を読み出す。
    pub get_event: unsafe extern "system" fn(This, i32, *mut Event) -> TResult,
    /// リストへイベントを追加する。本実装では呼び出さない。
    pub add_event: unsafe extern "system" fn(This, *mut Event) -> TResult,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOnEvent {
    /// イベント入力バス内のチャンネル番号。本実装では0..=15。
    pub channel: i16,
    /// 12平均律のノート番号（0..=127）。69が440 Hzに対応する。
    pub pitch: i16,
    /// ノート番号に加える音程補正（セント）。+100で半音上。
    pub tuning: f32,
    /// ノートオンの強さ（0..=1）。本実装の目標実効値は波形によらずこの値の0.5倍。
    pub velocity: f32,
    /// 省略可能なノート長（サンプル数）。本実装では使わず、Note Offで消音する。
    pub length: i32,
    /// ホストが割り当てるノート識別子。未提供を表す-1ではチャンネル・音高で照合する。
    pub note_id: i32,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NoteOffEvent {
    /// 終了対象ノートのチャンネル番号（0..=15）。
    pub channel: i16,
    /// 12平均律のノート番号（0..=127）。69が440 Hzに対応する。
    pub pitch: i16,
    /// ノートを離す強さ（0..=1）。本実装では参照しない。
    pub velocity: f32,
    /// ホストが割り当てるノート識別子。未提供を表す-1ではチャンネル・音高で照合する。
    pub note_id: i32,
    /// ノートオフ時の音程補正（セント）。本実装では参照しない。
    pub tuning: f32,
}

#[repr(C)]
pub union EventPayload {
    /// event_typeが0のときだけ読み出すNote Onのデータ。
    pub note_on: NoteOnEvent,
    /// event_typeが1のときだけ読み出すNote Offのデータ。
    pub note_off: NoteOffEvent,
    /// SDKのunion全体のサイズ・アラインメントを確保する領域。値としては読み出さない。
    /// SDKの全variantを格納可能な領域。最大はNoteExpressionTextEventの24バイト、
    /// アラインメント8。未対応variantは読み出さない。
    pub reserved: [u64; 3],
}

#[repr(C)]
pub struct Event {
    /// イベントバスの0始まりの番号。本実装は入力バス0だけを処理する。
    pub bus_index: i32,
    /// 現在のブロック先頭からのサンプル位置。この位置の波形生成前にイベントを反映する。
    pub sample_offset: i32,
    /// プロジェクト内のイベント位置（四分音符単位）。本実装では参照しない。
    pub ppq_position: f64,
    /// イベント属性のビットマスク。ライブ入力等を示すが、本実装では参照しない。
    pub flags: u16,
    /// payloadの種類。0はNote On、1はNote Off。それ以外は本実装では無視する。
    pub event_type: u16,
    /// event_typeに対応するイベント固有データ。ホスト由来の追加ポインターは保持しない。
    pub payload: EventPayload,
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

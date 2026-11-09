// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 最大32オシレータによる多声音VST3音源。公開するABIの実装はすべてRustで完結する。

#[cfg(not(all(
    target_arch = "x86_64",
    any(target_os = "linux", all(target_os = "windows", target_env = "msvc"))
)))]
compile_error!("対応対象はLinux x86_64とWindows x86_64 MSVCです");

mod abi;
mod distortion;
mod editor;
mod effects;
mod envelope;
mod eq;
mod filter;
mod float_mode;
mod gui;
mod host;
mod oscillator;
mod parameters;
mod plugin;
mod random;
mod synth;
mod voice;
mod volume;

use abi::*;
use std::ptr;
use std::sync::atomic::{AtomicU32, Ordering};

#[repr(C)]
struct Factory {
    /// 静的なファクトリー関数表。ABIで要求される先頭メンバー。
    vtable: &'static FactoryVTable,
    /// このFactoryの所有参照数。0になったときFactoryを破棄する。
    refs: AtomicU32,
}

static FACTORY_VTABLE: FactoryVTable = FactoryVTable {
    unknown: UnknownVTable {
        query_interface: factory_query,
        add_ref: factory_add_ref,
        release: factory_release,
    },
    get_factory_info: factory_info,
    count_classes,
    get_class_info: class_info,
    create_instance,
    get_class_info2: class_info2,
};

// SAFETY: ホストは有効な所有参照のthisと、各ABI型の読み書き可能な引数を渡す。
// nullは各入口で拒否するが、任意の不正アドレスを検証することはできない。
unsafe extern "system" fn factory_query(this: This, iid: *const Tuid, out: *mut This) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはホストが提供する出力領域。
    unsafe {
        out.write(ptr::null_mut());
    }
    if iid.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: iidは16バイトの識別子、thisは有効なFactory。
    unsafe {
        if matches!(*iid, UNKNOWN_IID | FACTORY_IID | FACTORY2_IID) {
            factory_add_ref(this);
            out.write(this);
            OK
        } else {
            NO_INTERFACE
        }
    }
}

unsafe extern "system" fn factory_add_ref(this: This) -> u32 {
    // SAFETY: thisはGetPluginFactoryから返された生存中のFactory。
    let factory = unsafe { &*this.cast::<Factory>() };
    retain(&factory.refs)
}

unsafe extern "system" fn factory_release(this: This) -> u32 {
    // SAFETY: 呼び出し元の所有参照がこの呼び出しの間Factoryを保持する。
    let refs = unsafe { &(*this.cast::<Factory>()).refs };
    let remaining = refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if remaining == 0 {
        // SAFETY: 最後の参照のみがBoxを回収し、この後thisを参照しない。
        unsafe {
            drop(Box::from_raw(this.cast::<Factory>()));
        }
    }
    remaining
}

pub(crate) fn retain(refs: &AtomicU32) -> u32 {
    let previous = refs.fetch_add(1, Ordering::Relaxed);
    if previous >= i32::MAX as u32 {
        std::process::abort();
    }
    previous + 1
}

unsafe extern "system" fn factory_info(_: This, out: *mut FactoryInfo) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはFactoryInfoの書き込み可能な領域。
    unsafe {
        out.write(FactoryInfo {
            vendor: ascii("sample-vsti"),
            url: [0; 256],
            email: [0; 128],
            flags: 1 << 4,
        });
    }
    OK
}

unsafe extern "system" fn count_classes(_: This) -> i32 {
    1
}

fn metadata() -> ClassInfo {
    ClassInfo {
        cid: COMPONENT_CID,
        cardinality: i32::MAX,
        category: ascii("Audio Module Class"),
        name: ascii("Sample VSTi"),
    }
}

unsafe extern "system" fn class_info(_: This, index: i32, out: *mut ClassInfo) -> TResult {
    if index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはClassInfoの書き込み可能な領域。
    unsafe {
        out.write(metadata());
    }
    OK
}

unsafe extern "system" fn class_info2(_: This, index: i32, out: *mut ClassInfo2) -> TResult {
    if index != 0 || out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはClassInfo2の書き込み可能な領域。
    unsafe {
        out.write(ClassInfo2 {
            base: metadata(),
            class_flags: 0,
            sub_categories: ascii("Instrument|Synth"),
            vendor: ascii("sample-vsti"),
            version: ascii(env!("SAMPLE_VSTI_VERSION")),
            sdk_version: ascii("VST 3.8.1"),
        });
    }
    OK
}

unsafe extern "system" fn create_instance(
    _: This,
    cid: *const Tuid,
    iid: *const Tuid,
    out: *mut This,
) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outはホストの出力領域。失敗時にもnullを返す。
    unsafe {
        out.write(ptr::null_mut());
    }
    if cid.is_null() || iid.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: cidとiidは16バイトの有効な入力識別子。
    unsafe {
        if *cid != COMPONENT_CID {
            return NO_INTERFACE;
        }
        plugin::create(&*iid, out)
    }
}

/// ホストへ初期所有参照を1つ返す。以後はFUnknown::releaseで解放する。
#[unsafe(no_mangle)]
pub extern "system" fn GetPluginFactory() -> This {
    Box::into_raw(Box::new(Factory {
        vtable: &FACTORY_VTABLE,
        refs: AtomicU32::new(1),
    }))
    .cast()
}

/// Linuxのモジュール開始。共有資源は持たない。
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn ModuleEntry(_: This) -> bool {
    true
}

/// ホストがすべての所有参照を解放した後に呼ぶ。
#[cfg(target_os = "linux")]
#[unsafe(no_mangle)]
pub extern "C" fn ModuleExit() -> bool {
    true
}

/// Windowsのモジュール開始。共有資源は持たないため複数回の呼び出しも許容する。
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "system" fn InitDll() -> bool {
    true
}

/// Windowsのモジュール終了。ホストが所有参照を解放した後に呼ぶ。
#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
pub extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(test)]
mod tests;

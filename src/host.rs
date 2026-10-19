// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! UIスレッドで所有するVST3参照。音声処理と別スレッドには渡さない。

use crate::abi::*;
use std::{marker::PhantomData, rc::Rc};

pub struct HostRef {
    /// addRef済み、またはqueryInterfaceが返した所有参照。
    pointer: This,
    /// UIスレッド以外へのSend/Syncを禁止する。
    ui_thread: PhantomData<Rc<()>>,
}

impl HostRef {
    /// SAFETY: nullでなければ生存中のFUnknown派生インターフェース。UIスレッド限定。
    pub unsafe fn retain(pointer: This) -> Option<Self> {
        if pointer.is_null() {
            return None;
        }
        // SAFETY: 呼び出し元が持つ参照から、このオブジェクトの独立した所有参照を作る。
        unsafe {
            ((&**pointer.cast::<*const UnknownVTable>()).add_ref)(pointer);
        }
        Some(Self {
            pointer,
            ui_thread: PhantomData,
        })
    }

    pub fn as_ptr(&self) -> This {
        self.pointer
    }

    #[cfg(target_os = "linux")]
    pub fn query(&self, iid: &Tuid) -> Option<Self> {
        let mut out = std::ptr::null_mut();
        // SAFETY: selfの所有参照が有効性を保証し、成功した出力は新しい所有参照。
        let status = unsafe {
            ((&**self.pointer.cast::<*const UnknownVTable>()).query_interface)(
                self.pointer,
                iid,
                &mut out,
            )
        };
        if status == OK && !out.is_null() {
            Some(Self {
                pointer: out,
                ui_thread: PhantomData,
            })
        } else {
            None
        }
    }
}

impl Clone for HostRef {
    fn clone(&self) -> Self {
        // SAFETY: selfは生存中の参照を所有する。
        unsafe { Self::retain(self.pointer) }.expect("owned VST3 reference is nonnull")
    }
}

impl Drop for HostRef {
    fn drop(&mut self) {
        // SAFETY: この値が保持する所有参照を一度だけ解放する。
        unsafe {
            ((&**self.pointer.cast::<*const UnknownVTable>()).release)(self.pointer);
        }
    }
}

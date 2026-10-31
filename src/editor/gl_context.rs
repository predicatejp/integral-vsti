// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! ホストと共有するWindows UIスレッドのカレントGLコンテキストを復元する。
use std::ffi::c_void;

#[link(name = "opengl32")]
unsafe extern "system" {
    fn wglGetCurrentDC() -> *mut c_void;
    fn wglGetCurrentContext() -> *mut c_void;
    fn wglMakeCurrent(dc: *mut c_void, context: *mut c_void) -> i32;
}

struct RestoreContextState {
    /// 呼び出し元の描画先。所有権は取得しない。
    dc: *mut c_void,
    /// 呼び出し元のGLコンテキスト。nullなら未設定へ戻す。
    context: *mut c_void,
}

/// 同じUIスレッドのスコープ終了時に復元する。生ポインターによりSend/Sync不可。
pub(crate) struct RestoreContext {
    state: RestoreContextState,
}

impl RestoreContext {
    pub(crate) fn capture() -> Self {
        // SAFETY: 呼び出しスレッドの設定を照会するだけで、所有権や寿命を変更しない。
        let state = unsafe {
            RestoreContextState {
                dc: wglGetCurrentDC(),
                context: wglGetCurrentContext(),
            }
        };
        Self { state }
    }
}

impl Drop for RestoreContext {
    fn drop(&mut self) {
        // SAFETY: 保存元と同じスレッドで、ホストへ戻る前に借用ハンドルを復元する。
        // ホストの資源は解放しない。OSの復元失敗で破棄処理をpanicさせない。
        unsafe {
            let _ = wglMakeCurrent(self.state.dc, self.state.context);
        }
    }
}

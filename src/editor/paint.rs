// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! Windowsの空矩形を伴う更新要求を完了させ、WM_PAINTの連続配送を防ぐ。
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{ffi::c_void, io};

#[repr(C)]
#[derive(Default)]
struct Rect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[repr(C)]
#[derive(Default)]
struct PaintStruct {
    dc: *mut c_void,
    erase: i32,
    rect: Rect,
    restore: i32,
    incremental_update: i32,
    reserved: [u8; 32],
}
const _: () = assert!(size_of::<PaintStruct>() == 72);
const _: () = assert!(std::mem::offset_of!(PaintStruct, rect) == 12);

type Callback = unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(window: *mut c_void, callback: Callback, id: usize, data: usize) -> i32;
    fn RemoveWindowSubclass(window: *mut c_void, callback: Callback, id: usize) -> i32;
    fn DefSubclassProc(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
}

#[link(name = "user32")]
unsafe extern "system" {
    fn GetUpdateRect(window: *mut c_void, rect: *mut Rect, erase: i32) -> i32;
    fn BeginPaint(window: *mut c_void, paint: *mut PaintStruct) -> *mut c_void;
    fn EndPaint(window: *mut c_void, paint: *const PaintStruct) -> i32;
}

struct PaintCompletionState {
    window: *mut c_void,
}

pub(super) struct PaintCompletion {
    // 安定したアドレスをsubclass IDに使う。生ポインターによりSend/Sync不可。
    state: Box<PaintCompletionState>,
}

impl PaintCompletion {
    pub(super) fn new(window: &baseview::WindowContext) -> io::Result<Self> {
        let handle = window
            .window_handle()
            .map_err(|_| io::Error::other("Window handle unavailable"))?;
        let RawWindowHandle::Win32(handle) = handle.as_raw() else {
            return Err(io::Error::other("Win32 window required"));
        };
        let completion = Self {
            state: Box::new(PaintCompletionState {
                window: handle.hwnd.get() as *mut c_void,
            }),
        };
        // SAFETY: 作成元のUIスレッド上の子ウィンドウだけを対象とする。
        // IDは識別にだけ使い、コールバックからStateを参照しない。
        if unsafe { SetWindowSubclass(completion.state.window, callback, completion.id(), 0) } == 0
        {
            return Err(io::Error::other(
                "Could not install paint completion handler",
            ));
        }
        Ok(completion)
    }

    fn id(&self) -> usize {
        &*self.state as *const PaintCompletionState as usize
    }
}

impl Drop for PaintCompletion {
    fn drop(&mut self) {
        // SAFETY: 同じUIスレッドで、所有するIDだけをStateの解放前に取り外す。
        // ウィンドウが既に破棄されていても、再利用された別のIDへ影響しない。
        unsafe { RemoveWindowSubclass(self.state.window, callback, self.id()) };
    }
}

unsafe extern "system" fn callback(
    window: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    id: usize,
    _: usize,
) -> isize {
    // SAFETY: 引数はOSから受け取り、同じウィンドウの下位処理へそのまま渡す。
    if message == 0x0082 {
        // WM_NCDESTROY。ウィンドウの破棄時にもこのsubclassだけを解除する。
        unsafe { RemoveWindowSubclass(window, callback, id) };
    }
    let result = unsafe { DefSubclassProc(window, message, wparam, lparam) };
    if message == 0x000f {
        // baseviewは空矩形のWM_PAINTを描画もvalidateもせず返す。
        // 更新あり・空矩形の組合せも完了させる。描画中に追加された非空領域は残す。
        let mut rect = Rect::default();
        // SAFETY: 書込み可能なRECTを渡し、消去の再入を要求しない。
        if unsafe { GetUpdateRect(window, &mut rect, 0) } != 0
            && (rect.left >= rect.right || rect.top >= rect.bottom)
        {
            let mut paint = PaintStruct::default();
            // SAFETY: WM_PAINTの処理中に、この子ウィンドウの要求だけを完了する。
            // 空のRECTをValidateRectへ渡すと何も取り除かれないため、
            // Windows標準のBeginPaint/EndPaintで更新領域を消費する。
            if !unsafe { BeginPaint(window, &mut paint) }.is_null() {
                unsafe { EndPaint(window, &paint) };
            }
        }
    }
    result
}

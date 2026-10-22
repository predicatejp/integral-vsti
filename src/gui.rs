// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! VST3 IPlugViewを直接公開するeguiエディター。音声処理にはアクセスしない。

use crate::{abi::*, host::HostRef, parameters::SharedParameters};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, c_void},
    ptr,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};

const WIDTH: i32 = 1024;
const HEIGHT: i32 = 768;
const RECT: ViewRect = ViewRect {
    left: 0,
    top: 0,
    right: WIDTH,
    bottom: HEIGHT,
};

#[cfg(target_os = "windows")]
thread_local! {
    // Windowsのbuilderは呼び出し元UIスレッドで実行される。RcをSend境界に渡さない。
    static VIEWS: RefCell<std::collections::HashMap<usize, std::rc::Weak<EditorView>>> = RefCell::new(std::collections::HashMap::new());
}
#[cfg(target_os = "windows")]
const PLATFORM: &[u8] = b"HWND";
#[cfg(target_os = "linux")]
const PLATFORM: &[u8] = b"X11EmbedWindowID";

#[repr(C)]
struct Interface {
    /// 公開インターフェースの関数表。
    table: *const c_void,
    /// 最後のreleaseまで有効な埋め込み元。
    owner: *mut View,
}
struct View {
    /// IPlugViewの公開口。
    view: Interface,
    #[cfg(target_os = "linux")]
    /// Linux::ITimerHandlerの公開口。
    timer: Interface,
    /// すべての公開口で共有する参照数。
    refs: AtomicU32,
    /// UIスレッド限定の画面状態。再入の間もRcで生存させる。
    state: Rc<EditorView>,
}
struct ViewState {
    /// Controllerの所有参照。画面が存在する間インスタンスを保持する。
    controller: HostRef,
    #[cfg(target_os = "linux")]
    /// IRunLoopに登録するITimerHandlerの借用アドレス。
    timer_interface: Cell<This>,
    /// LinuxのIRunLoop取得元。UIスレッドでだけ読み書きする。
    frame: RefCell<Option<HostRef>>,
    /// ネイティブ子ウィンドウ。削除すると描画スレッドも停止・回収される。
    window: RefCell<Option<Rc<baseview::Window>>>,
    #[cfg(target_os = "linux")]
    /// UIスレッドで編集を配送するタイマー。取り外し時に必ず解除する。
    ticker: RefCell<Option<Ticker>>,
    /// Controllerと共有する14個の正規化値。
    parameters: Arc<SharedParameters>,
    /// 描画側からの操作の送り口。音声スレッドでは使用しない。
    sender: SyncSender<(usize, f64)>,
    /// 操作の受け口。ホストUIスレッドだけで取り出す。
    receiver: RefCell<Receiver<(usize, f64)>>,
    /// ホストコールバックによる再入で二重配送しないためのフラグ。
    polling: Cell<bool>,
}

/// UIスレッドで画面状態の更新と編集配送を担当する。
pub(super) struct EditorView {
    /// 所有資源と配送状況。処理はEditorViewのメソッドに置く。
    state: ViewState,
}

impl EditorView {
    pub(super) fn poll(&self) {
        if self.state.polling.replace(true) {
            return;
        }
        let window = self.state.window.borrow().clone();
        if let Some(_window) = window {
            for _ in 0..32 {
                let edit = self.state.receiver.borrow().try_recv();
                let Ok((index, value)) = edit else {
                    break;
                };
                // SAFETY: 両OSのタイマーはホストUIスレッドでのみ配送する。controllerが所有参照を保持する。
                unsafe {
                    crate::plugin::edit_parameter(self.state.controller.as_ptr(), index, value);
                }
            }
        }
        self.state.polling.set(false);
    }
    fn close(&self) {
        #[cfg(target_os = "windows")]
        VIEWS.with(|views| {
            views
                .borrow_mut()
                .remove(&(std::ptr::from_ref(self) as usize))
        });
        // 外部への呼び出し中にRefCellを借用し続けない。
        #[cfg(target_os = "linux")]
        {
            let ticker = self.state.ticker.borrow_mut().take();
            drop(ticker);
        }
        let window = self.state.window.borrow_mut().take();
        drop(window);
        while self.state.receiver.borrow().try_recv().is_ok() {}
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        self.close();
    }
}

struct Parent(RawWindowHandle);
impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        // SAFETY: attachedの親はremovedまでホストが保持し、親の所有権は移さない。
        Ok(unsafe { WindowHandle::borrow_raw(self.0) })
    }
}

const UNKNOWN: UnknownVTable = UnknownVTable {
    query_interface: query,
    add_ref,
    release,
};
static VIEW_TABLE: ViewVTable = ViewVTable {
    unknown: UNKNOWN,
    is_platform_type_supported: platform,
    attached,
    removed,
    on_wheel: wheel,
    on_key_down: key,
    on_key_up: key,
    get_size,
    on_size,
    on_focus: focus,
    set_frame,
    can_resize: cannot_resize,
    check_size_constraint: get_size,
};
#[cfg(target_os = "linux")]
static TIMER_TABLE: TimerVTable = TimerVTable {
    unknown: UNKNOWN,
    on_timer: tick,
};

/// SAFETY: Controllerへの有効な借用参照をUIスレッドで渡す。
pub unsafe fn create(controller: This, parameters: Arc<SharedParameters>) -> This {
    let Some(controller) = (unsafe { HostRef::retain(controller) }) else {
        return ptr::null_mut();
    };
    let (sender, receiver) = mpsc::sync_channel(32);
    let state = Rc::new(EditorView {
        state: ViewState {
            controller,
            #[cfg(target_os = "linux")]
            timer_interface: Cell::new(ptr::null_mut()),
            frame: RefCell::new(None),
            window: RefCell::new(None),
            #[cfg(target_os = "linux")]
            ticker: RefCell::new(None),
            parameters,
            sender,
            receiver: RefCell::new(receiver),
            polling: Cell::new(false),
        },
    });
    let owner = Box::into_raw(Box::new(View {
        view: Interface {
            table: ptr::from_ref(&VIEW_TABLE).cast(),
            owner: ptr::null_mut(),
        },
        #[cfg(target_os = "linux")]
        timer: Interface {
            table: ptr::from_ref(&TIMER_TABLE).cast(),
            owner: ptr::null_mut(),
        },
        refs: AtomicU32::new(1),
        state,
    }));
    // SAFETY: 公開前に所有者を設定。以後インターフェースのアドレスは固定する。
    unsafe {
        (*owner).view.owner = owner;
        let view = ptr::addr_of_mut!((*owner).view).cast();
        #[cfg(target_os = "linux")]
        {
            (*owner).timer.owner = owner;
            let state = &(*owner).state;
            state
                .state
                .timer_interface
                .set(ptr::addr_of_mut!((*owner).timer).cast());
        }
        view
    }
}

// SAFETY契約: このモジュールが公開した生存中のInterface。UI操作はホストがUIスレッドへ直列化する。
unsafe fn state(this: This) -> Rc<EditorView> {
    unsafe { Rc::clone(&(*(*this.cast::<Interface>()).owner).state) }
}
unsafe extern "system" fn query(this: This, iid: *const Tuid, out: *mut This) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: outは出力領域、iidはnull以外なら有効な識別子。
    unsafe {
        out.write(ptr::null_mut());
        if iid.is_null() {
            return INVALID_ARGUMENT;
        }
        let owner = (*this.cast::<Interface>()).owner;
        let result = match *iid {
            UNKNOWN_IID | VIEW_IID => ptr::addr_of_mut!((*owner).view).cast(),
            #[cfg(target_os = "linux")]
            TIMER_IID => ptr::addr_of_mut!((*owner).timer).cast(),
            _ => return NO_INTERFACE,
        };
        add_ref(this);
        out.write(result);
    }
    OK
}
unsafe extern "system" fn add_ref(this: This) -> u32 {
    // SAFETY: 所有参照がView全体を保持する。
    crate::retain(unsafe { &(*(*this.cast::<Interface>()).owner).refs })
}
unsafe extern "system" fn release(this: This) -> u32 {
    // SAFETY: 最後の所有参照だけがBoxを回収。UI資源の最終解放はUIスレッドで行う。
    unsafe {
        let owner = (*this.cast::<Interface>()).owner;
        let count = (*owner).refs.fetch_sub(1, Ordering::AcqRel) - 1;
        if count == 0 {
            drop(Box::from_raw(owner));
        }
        count
    }
}
unsafe extern "system" fn platform(_: This, kind: *const u8) -> TResult {
    // SAFETY: kindはnullかNUL終端文字列。
    if !kind.is_null() && unsafe { CStr::from_ptr(kind.cast()) }.to_bytes() == PLATFORM {
        OK
    } else {
        FALSE
    }
}
unsafe extern "system" fn attached(this: This, parent: This, kind: *const u8) -> TResult {
    if parent.is_null() || unsafe { platform(this, kind) } != OK {
        return INVALID_ARGUMENT;
    }
    let state = unsafe { state(this) };
    if state.state.window.borrow().is_some() {
        return FALSE;
    }
    #[cfg(target_os = "windows")]
    let parent = Parent(RawWindowHandle::Win32(
        raw_window_handle::Win32WindowHandle::new(
            std::num::NonZeroIsize::new(parent as isize).expect("nonnull parent"),
        ),
    ));
    #[cfg(target_os = "linux")]
    let parent = {
        if parent as usize > u32::MAX as usize {
            return INVALID_ARGUMENT;
        }
        Parent(RawWindowHandle::Xcb(
            raw_window_handle::XcbWindowHandle::new(
                std::num::NonZeroU32::new(parent as usize as u32).expect("nonnull parent"),
            ),
        ))
    };
    #[cfg(target_os = "linux")]
    {
        let Ok(ticker) = Ticker::new(&state) else {
            return FALSE;
        };
        *state.state.ticker.borrow_mut() = Some(ticker);
    }
    #[cfg(target_os = "windows")]
    let key = {
        let key = Rc::as_ptr(&state) as usize;
        VIEWS.with(|views| views.borrow_mut().insert(key, Rc::downgrade(&state)));
        key
    };
    let settings = baseview::WindowSettings::new()
        .with_title("Sample VSTi")
        .with_size(baseview::dpi::PhysicalSize::new(
            WIDTH as u32,
            HEIGHT as u32,
        ))
        .with_resizable(false)
        .with_parent(Some(&parent))
        .with_gl_config(Some(baseview::gl::GlConfig::default()));
    let parameters = Arc::clone(&state.state.parameters);
    let edits = state.state.sender.clone();
    match baseview::Window::create(settings, move |window| {
        #[cfg(target_os = "windows")]
        let view = VIEWS
            .with(|views| views.borrow().get(&key).cloned())
            .ok_or_else(|| std::io::Error::other("view closed"))?;
        super::editor::Editor::new(
            window,
            parameters,
            edits,
            #[cfg(target_os = "windows")]
            view,
        )
    }) {
        Ok(window) => {
            let window = Rc::new(window);
            *state.state.window.borrow_mut() = Some(Rc::clone(&window));
            if window.show().is_err() {
                state.close();
                return FALSE;
            }
            OK
        }
        Err(_) => {
            state.close();
            FALSE
        }
    }
}
unsafe extern "system" fn removed(this: This) -> TResult {
    // SAFETY: UIスレッド。DSPには触れず、GUIの借用と登録だけを終了する。
    unsafe { state(this) }.close();
    OK
}
unsafe extern "system" fn wheel(_: This, _: f32) -> TResult {
    FALSE
}
unsafe extern "system" fn key(_: This, _: u16, _: i16, _: i16) -> TResult {
    FALSE
}
unsafe extern "system" fn focus(_: This, _: u8) -> TResult {
    OK
}
unsafe extern "system" fn cannot_resize(_: This) -> TResult {
    FALSE
}
unsafe extern "system" fn get_size(_: This, out: *mut ViewRect) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストの出力領域へ現在のサイズを返す。
    unsafe {
        out.write(RECT);
    }
    OK
}
unsafe extern "system" fn on_size(_: This, rect: *const ViewRect) -> TResult {
    if rect.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストから渡された有効なViewRect。固定サイズ以外は受理しない。
    let rect = unsafe { *rect };
    if rect.right.checked_sub(rect.left) == Some(WIDTH)
        && rect.bottom.checked_sub(rect.top) == Some(HEIGHT)
    {
        OK
    } else {
        FALSE
    }
}
unsafe extern "system" fn set_frame(this: This, frame: This) -> TResult {
    let state = unsafe { state(this) };
    // 埋め込み中の接続変更は受理しない。removed後は交換・解除できる。
    if state.state.window.borrow().is_some() {
        return FALSE;
    }
    // SAFETY: nullか有効なIPlugFrame。交換前に新しい参照を確保する。
    let new = unsafe { HostRef::retain(frame) };
    let old = state.state.frame.replace(new);
    drop(old);
    OK
}

#[cfg(target_os = "linux")]
struct Ticker {
    /// 登録先IRunLoopの所有参照。
    run_loop: HostRef,
    /// 登録したITimerHandler。所有するViewはホストの登録期間中保持される。
    handler: This,
}
#[cfg(target_os = "linux")]
impl Ticker {
    fn new(state: &Rc<EditorView>) -> Result<Self, ()> {
        let frame = state.state.frame.borrow().clone().ok_or(())?;
        let run_loop = frame.query(&RUN_LOOP_IID).ok_or(())?;
        let handler = state.state.timer_interface.get();
        let p = run_loop.as_ptr();
        // SAFETY: 有効なIRunLoopと、取り外しまで有効なITimerHandler。
        if unsafe { ((&**p.cast::<*const RunLoopVTable>()).register_timer)(p, handler, 16) } != OK {
            return Err(());
        }
        Ok(Self { run_loop, handler })
    }
}
#[cfg(target_os = "linux")]
impl Drop for Ticker {
    fn drop(&mut self) {
        let p = self.run_loop.as_ptr();
        // SAFETY: 登録先を所有し、登録解除完了までhandlerを破棄しない。
        unsafe {
            ((&**p.cast::<*const RunLoopVTable>()).unregister_timer)(p, self.handler);
        }
    }
}
#[cfg(target_os = "linux")]
unsafe extern "system" fn tick(this: This) {
    // SAFETY: IRunLoopはホストのUIスレッドからだけ呼ぶ。
    unsafe { state(this) }.poll();
}

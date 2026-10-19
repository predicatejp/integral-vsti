// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! VST3 IPlugViewを直接公開するeguiエディター。音声処理にはアクセスしない。

use crate::{abi::*, host::HostRef, parameters::WAVEFORM_NAMES};
use egui_baseview::{App, EguiWindow, EguiWindowSettings, Frame, KeyCapture, baseview};
use raw_window_handle::{HandleError, HasWindowHandle, RawWindowHandle, WindowHandle};
use std::{
    cell::{Cell, RefCell},
    ffi::{CStr, c_void},
    ptr,
    rc::{Rc, Weak},
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
    time::Duration,
};

const WIDTH: i32 = 360;
const HEIGHT: i32 = 180;
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
    /// IPlugViewContentScaleSupportの公開口。
    scale: Interface,
    #[cfg(target_os = "linux")]
    /// Linux::ITimerHandlerの公開口。
    timer: Interface,
    /// すべての公開口で共有する参照数。
    refs: AtomicU32,
    /// UIスレッド限定の画面状態。再入の間もRcで生存させる。
    state: Rc<ViewState>,
}
struct ViewState {
    /// Controllerの所有参照。画面が存在する間インスタンスを保持する。
    controller: HostRef,
    /// ホストへresizeViewで渡すIPlugView。所有者Viewの生存中だけ使用する。
    view: Cell<This>,
    #[cfg(target_os = "linux")]
    /// IRunLoopに登録するITimerHandlerの借用アドレス。
    timer_interface: Cell<This>,
    /// ホストのサイズ変更先。UIスレッドでだけ読み書きする。
    frame: RefCell<Option<HostRef>>,
    /// ネイティブ子ウィンドウ。削除すると描画スレッドも停止・回収される。
    window: RefCell<Option<Rc<baseview::Window>>>,
    /// UIスレッドで編集を配送するタイマー。取り外し時に必ず解除する。
    ticker: RefCell<Option<Ticker>>,
    /// ホストに承認された現在の物理サイズ。
    rect: Cell<ViewRect>,
    /// ホストまたはOSが指定した表示倍率。描画側はatomic経由で読む。
    scale: Arc<AtomicU32>,
    /// Controllerと共有する現在の波形番号。
    waveform: Arc<AtomicU32>,
    /// 描画側からの操作の送り口。音声スレッドでは使用しない。
    sender: SyncSender<u32>,
    /// 操作の受け口。ホストUIスレッドだけで取り出す。
    receiver: RefCell<Receiver<u32>>,
    /// ホストコールバックによる再入で二重配送しないためのフラグ。
    polling: Cell<bool>,
}

struct EditorApp {
    /// Controllerの表示値。ホスト由来の更新も再描画時に反映する。
    waveform: Arc<AtomicU32>,
    /// 目的の物理ピクセル倍率。
    scale: Arc<AtomicU32>,
    /// ホストUIスレッドへの有限長キュー。GUI専用でDSPとは共有しない。
    edits: SyncSender<u32>,
}
impl App for EditorApp {
    fn build(&mut self, _: egui::Context, frame: &mut Frame) -> Result<(), baseview::HandlerError> {
        frame.set_key_capture(KeyCapture::IgnoreAll);
        Ok(())
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut Frame) {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        let native_scale = frame.baseview_window().size().scale_factor as f32;
        ui.ctx().set_zoom_factor(scale / native_scale);
        // 表示値の同期はUI内で行う。DSP・ホストへの再通知は発生させない。
        ui.ctx().request_repaint_after(Duration::from_millis(33));
        egui::Frame::new()
            .inner_margin(12)
            .fill(ui.visuals().panel_fill)
            .show(ui, |ui| {
                ui.set_min_size(ui.available_size());
                ui.add_space(14.0);
                ui.heading("Sample VSTi");
                ui.label("Waveform");
                ui.add_space(12.0);
                let selected = self.waveform.load(Ordering::Relaxed);
                ui.horizontal(|ui| {
                    for (index, name) in WAVEFORM_NAMES.iter().enumerate() {
                        let button = egui::Button::new(*name).selected(index as u32 == selected);
                        let response = ui.add_sized([78.0, 36.0], button);
                        // 初版はマウス操作のみ。OS経由のキーでパラメーターを編集しない。
                        ui.memory_mut(|memory| memory.surrender_focus(response.id));
                        if response.clicked() && response.interact_pointer_pos().is_some() {
                            let _ = self.edits.try_send(index as u32);
                        }
                    }
                });
            });
    }
}

impl ViewState {
    fn desired_rect(&self) -> ViewRect {
        let scale = f32::from_bits(self.scale.load(Ordering::Relaxed));
        ViewRect {
            left: 0,
            top: 0,
            right: (WIDTH as f32 * scale).round() as i32,
            bottom: (HEIGHT as f32 * scale).round() as i32,
        }
    }
    fn poll(&self) {
        if self.polling.replace(true) {
            return;
        }
        let window = self.window.borrow().clone();
        if let Some(window) = window {
            window.host_main_thread_callback();
            for _ in 0..32 {
                let edit = self.receiver.borrow().try_recv();
                let Ok(index) = edit else {
                    break;
                };
                // SAFETY: TickerはホストUIスレッドでのみ呼ぶ。controllerが所有参照を保持する。
                unsafe {
                    crate::plugin::edit_waveform(self.controller.as_ptr(), index);
                }
            }
        }
        self.polling.set(false);
    }
    fn close(&self) {
        // 外部への呼び出し中にRefCellを借用し続けない。
        let ticker = self.ticker.borrow_mut().take();
        drop(ticker);
        let window = self.window.borrow_mut().take();
        drop(window);
        while self.receiver.borrow().try_recv().is_ok() {}
    }
    fn resize(&self, factor: f32) -> TResult {
        if !factor.is_finite() || !(0.5..=8.0).contains(&factor) {
            return INVALID_ARGUMENT;
        }
        let old = self.scale.swap(factor.to_bits(), Ordering::Relaxed);
        let mut rect = self.desired_rect();
        let frame = self.frame.borrow().clone();
        if let Some(frame) = frame {
            // SAFETY: 所有参照を保持し、UIスレッドから通知する。onSizeの再入を許す。
            let status = unsafe {
                let p = frame.as_ptr();
                ((&**p.cast::<*const FrameVTable>()).resize_view)(p, self.view.get(), &mut rect)
            };
            if status != OK {
                self.scale.store(old, Ordering::Relaxed);
                return status;
            }
        } else {
            self.rect.set(rect);
        }
        OK
    }
}
impl Drop for ViewState {
    fn drop(&mut self) {
        self.close();
    }
}

struct Callbacks(Weak<ViewState>);
impl baseview::host::HostCallbacks for Callbacks {
    fn request_resize(&mut self, size: baseview::WindowSize) -> Result<(), baseview::HandlerError> {
        let Some(state) = self.0.upgrade() else {
            return Err(std::io::Error::other("view closed").into());
        };
        if state.resize(size.scale_factor as f32) == OK {
            Ok(())
        } else {
            Err(std::io::Error::other("host rejected resize").into())
        }
    }
    fn destroyed(&mut self) {}
}
struct MainThreadWake;
impl baseview::host::HostMainThreadCaller for MainThreadWake {
    // IRunLoopの周期タイマーで要求を回収する。描画スレッドからホストを呼ばない。
    fn call_main_thread(&mut self) {}
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
    check_size_constraint: constrain,
};
static SCALE_TABLE: ScaleVTable = ScaleVTable {
    unknown: UNKNOWN,
    set_content_scale_factor: scale,
};
#[cfg(target_os = "linux")]
static TIMER_TABLE: TimerVTable = TimerVTable {
    unknown: UNKNOWN,
    on_timer: tick,
};

/// SAFETY: Controllerへの有効な借用参照をUIスレッドで渡す。
pub unsafe fn create(controller: This, waveform: Arc<AtomicU32>) -> This {
    let Some(controller) = (unsafe { HostRef::retain(controller) }) else {
        return ptr::null_mut();
    };
    let (sender, receiver) = mpsc::sync_channel(32);
    let state = Rc::new(ViewState {
        controller,
        view: Cell::new(ptr::null_mut()),
        #[cfg(target_os = "linux")]
        timer_interface: Cell::new(ptr::null_mut()),
        frame: RefCell::new(None),
        window: RefCell::new(None),
        ticker: RefCell::new(None),
        rect: Cell::new(ViewRect {
            left: 0,
            top: 0,
            right: WIDTH,
            bottom: HEIGHT,
        }),
        scale: Arc::new(AtomicU32::new(1.0f32.to_bits())),
        waveform,
        sender,
        receiver: RefCell::new(receiver),
        polling: Cell::new(false),
    });
    let owner = Box::into_raw(Box::new(View {
        view: Interface {
            table: ptr::from_ref(&VIEW_TABLE).cast(),
            owner: ptr::null_mut(),
        },
        scale: Interface {
            table: ptr::from_ref(&SCALE_TABLE).cast(),
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
        (*owner).scale.owner = owner;
        let view = ptr::addr_of_mut!((*owner).view).cast();
        let state = &(*owner).state;
        state.view.set(view);
        #[cfg(target_os = "linux")]
        {
            (*owner).timer.owner = owner;
            state
                .timer_interface
                .set(ptr::addr_of_mut!((*owner).timer).cast());
        }
        view
    }
}

// SAFETY契約: このモジュールが公開した生存中のInterface。UI操作はホストがUIスレッドへ直列化する。
unsafe fn state(this: This) -> Rc<ViewState> {
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
            SCALE_IID => ptr::addr_of_mut!((*owner).scale).cast(),
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
    if state.window.borrow().is_some() {
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
    let Ok(ticker) = Ticker::new(&state) else {
        return FALSE;
    };
    *state.ticker.borrow_mut() = Some(ticker);
    let rect = state.desired_rect();
    let settings = EguiWindowSettings::new()
        .with_title("Sample VSTi")
        .with_size(baseview::dpi::PhysicalSize::new(
            rect.right as u32,
            rect.bottom as u32,
        ))
        .with_resizable(false)
        .with_parent(Some(&parent));
    let host = baseview::host::Host::new()
        .with_main_thread(MainThreadWake)
        .with_callbacks(Callbacks(Rc::downgrade(&state)));
    let app = EditorApp {
        waveform: Arc::clone(&state.waveform),
        scale: Arc::clone(&state.scale),
        edits: state.sender.clone(),
    };
    match EguiWindow::create_with_host(settings, app, Some(host)) {
        Ok(window) => {
            let window = Rc::new(window);
            *state.window.borrow_mut() = Some(Rc::clone(&window));
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
unsafe extern "system" fn get_size(this: This, out: *mut ViewRect) -> TResult {
    if out.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストの出力領域へ現在のサイズを返す。
    unsafe {
        out.write(state(this).rect.get());
    }
    OK
}
unsafe extern "system" fn on_size(this: This, rect: *const ViewRect) -> TResult {
    if rect.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: ホストのViewRectを読み、子ウィンドウだけを変更する。
    let (state, rect) = unsafe { (state(this), *rect) };
    let wanted = state.desired_rect();
    if rect.right.checked_sub(rect.left) != Some(wanted.right)
        || rect.bottom.checked_sub(rect.top) != Some(wanted.bottom)
    {
        return FALSE;
    }
    let window = state.window.borrow().clone();
    if let Some(window) = window
        && window
            .resize(baseview::dpi::PhysicalSize::new(
                wanted.right as u32,
                wanted.bottom as u32,
            ))
            .is_err()
    {
        return FALSE;
    }
    state.rect.set(rect);
    OK
}
unsafe extern "system" fn constrain(this: This, rect: *mut ViewRect) -> TResult {
    if rect.is_null() {
        return INVALID_ARGUMENT;
    }
    // SAFETY: 出力領域を固定の論理サイズ×現在の倍率に制限する。
    unsafe {
        rect.write(state(this).desired_rect());
    }
    OK
}
unsafe extern "system" fn scale(this: This, factor: f32) -> TResult {
    // SAFETY: UIスレッドでホストへ通知する。
    unsafe { state(this) }.resize(factor)
}
unsafe extern "system" fn set_frame(this: This, frame: This) -> TResult {
    let state = unsafe { state(this) };
    // 埋め込み中の接続変更は受理しない。removed後は交換・解除できる。
    if state.window.borrow().is_some() {
        return FALSE;
    }
    // SAFETY: nullか有効なIPlugFrame。交換前に新しい参照を確保する。
    let new = unsafe { HostRef::retain(frame) };
    let old = state.frame.replace(new);
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
    fn new(state: &Rc<ViewState>) -> Result<Self, ()> {
        let frame = state.frame.borrow().clone().ok_or(())?;
        let run_loop = frame.query(&RUN_LOOP_IID).ok_or(())?;
        let handler = state.timer_interface.get();
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

#[cfg(target_os = "windows")]
mod windows_timer {
    use super::*;
    use windows_sys::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
    thread_local! {
        static TIMERS: RefCell<std::collections::HashMap<usize, Weak<ViewState>>> = RefCell::new(std::collections::HashMap::new());
    }
    pub(super) struct Ticker {
        /// このUIスレッドに登録したWin32タイマーID。
        id: usize,
    }
    impl Ticker {
        pub(super) fn new(state: &Rc<ViewState>) -> Result<Self, ()> {
            // SAFETY: 現在のホストUIスレッドのメッセージループへ登録する。
            let id = unsafe { SetTimer(ptr::null_mut(), 0, 16, Some(tick)) };
            if id == 0 {
                return Err(());
            }
            TIMERS.with(|timers| timers.borrow_mut().insert(id, Rc::downgrade(state)));
            Ok(Self { id })
        }
    }
    impl Drop for Ticker {
        fn drop(&mut self) {
            TIMERS.with(|timers| timers.borrow_mut().remove(&self.id));
            // SAFETY: 登録した同じUIスレッドで解除する。遅延メッセージはmapから取得できず無視される。
            unsafe {
                KillTimer(ptr::null_mut(), self.id);
            }
        }
    }
    unsafe extern "system" fn tick(_: This, _: u32, id: usize, _: u32) {
        let state = TIMERS.with(|timers| timers.borrow().get(&id).and_then(Weak::upgrade));
        if let Some(state) = state {
            state.poll();
        }
    }
}
#[cfg(target_os = "windows")]
use windows_timer::Ticker;

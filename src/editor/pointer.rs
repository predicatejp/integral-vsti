// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! ノブのドラッグ中だけ使うOS操作。カーソルの移動範囲は拘束しない。
use super::Editor;
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::ffi::c_void;

#[cfg(target_os = "windows")]
mod native {
    use super::*;
    #[repr(C)]
    struct Point {
        x: i32,
        y: i32,
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetCursorPos(point: *mut Point) -> i32;
        fn ScreenToClient(window: *mut c_void, point: *mut Point) -> i32;
        fn ClientToScreen(window: *mut c_void, point: *mut Point) -> i32;
        fn SetCursorPos(x: i32, y: i32) -> i32;
        fn GetCapture() -> *mut c_void;
        fn GetFocus() -> *mut c_void;
        fn GetForegroundWindow() -> *mut c_void;
        fn ReleaseCapture() -> i32;
        fn GetAsyncKeyState(key: i32) -> i16;
        fn SetCursor(cursor: *mut c_void) -> *mut c_void;
        fn LoadCursorW(instance: *mut c_void, name: *const u16) -> *mut c_void;
    }
    impl Editor {
        pub(in crate::editor) fn keyboard_focus(&self) -> [usize; 2] {
            // SAFETY: UIスレッドのフォーカスと前面ウィンドウを照会するだけで変更しない。
            unsafe { [GetFocus() as usize, GetForegroundWindow() as usize] }
        }

        pub(in crate::editor) fn drag_escape_pressed(&self) -> bool {
            // SAFETY: ノブ操作中だけEscの現在の押下状態を参照する。
            unsafe { GetAsyncKeyState(0x1b) < 0 }
        }

        fn hwnd(&self) -> Option<*mut c_void> {
            match self.window.window_handle().ok()?.as_raw() {
                RawWindowHandle::Win32(handle) => Some(handle.hwnd.get() as *mut c_void),
                _ => None,
            }
        }
        pub(in crate::editor) fn drag_has_focus(&self) -> bool {
            self.state
                .borrow()
                .drag
                .as_ref()
                .is_some_and(|drag| drag.keyboard_focus == self.keyboard_focus())
        }
        pub(in crate::editor) fn drag_pointer_position(&self) -> Option<egui::Pos2> {
            let window = self.hwnd()?;
            let mut point = Point { x: 0, y: 0 };
            // SAFETY: 描画/UIスレッドの生存中の子ウィンドウと書き込み可能なPOINT。
            unsafe {
                if GetCapture() != window
                    || GetAsyncKeyState(1) >= 0
                    || GetCursorPos(&mut point) == 0
                    || ScreenToClient(window, &mut point) == 0
                {
                    return None;
                }
            }
            Some(egui::pos2(point.x as f32, point.y as f32))
        }
        pub(in crate::editor) fn hide_drag_pointer(&self) -> bool {
            // SAFETY: baseviewがボタン押下時に確保したキャプチャの期間だけ非表示にする。
            // ShowCursorの表示カウンターは変更しない。
            unsafe {
                SetCursor(std::ptr::null_mut());
            }
            true
        }
        pub(in crate::editor) fn warp_drag_pointer(&self, position: egui::Pos2) -> bool {
            let Some(window) = self.hwnd() else {
                return false;
            };
            let mut point = Point {
                x: position.x as i32,
                y: position.y as i32,
            };
            // SAFETY: 上と同じUIスレッド。クライアント座標から画面座標へ変換する。
            unsafe {
                ClientToScreen(window, &mut point) != 0 && SetCursorPos(point.x, point.y) != 0
            }
        }
        pub(in crate::editor) fn restore_drag_pointer(&self) {
            // SAFETY: 自分のキャプチャだけ解除する。標準カーソルは共有資源で破棄不要。
            unsafe {
                if self.hwnd().is_some_and(|window| GetCapture() == window) {
                    ReleaseCapture();
                }
                SetCursor(LoadCursorW(std::ptr::null_mut(), 32512usize as *const u16));
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod native {
    use super::*;
    use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
    use std::ffi::{c_char, c_int, c_uint, c_ulong};
    #[repr(C)]
    struct Color {
        pixel: c_ulong,
        red: u16,
        green: u16,
        blue: u16,
        flags: c_char,
        pad: c_char,
    }
    #[link(name = "libX11.so.6", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        fn XGetInputFocus(d: *mut c_void, focus: *mut c_ulong, revert: *mut c_int) -> c_int;
        fn XSetInputFocus(d: *mut c_void, focus: c_ulong, revert: c_int, time: c_ulong) -> c_int;
        fn XQueryTree(
            d: *mut c_void,
            w: c_ulong,
            root: *mut c_ulong,
            parent: *mut c_ulong,
            children: *mut *mut c_ulong,
            count: *mut c_uint,
        ) -> c_int;
        fn XFree(data: *mut c_void) -> c_int;
        fn XQueryPointer(
            d: *mut c_void,
            w: c_ulong,
            root: *mut c_ulong,
            child: *mut c_ulong,
            rx: *mut c_int,
            ry: *mut c_int,
            x: *mut c_int,
            y: *mut c_int,
            mask: *mut c_uint,
        ) -> c_int;
        fn XWarpPointer(
            d: *mut c_void,
            src: c_ulong,
            dst: c_ulong,
            sx: c_int,
            sy: c_int,
            width: c_uint,
            height: c_uint,
            x: c_int,
            y: c_int,
        ) -> c_int;
        fn XCreateBitmapFromData(
            d: *mut c_void,
            w: c_ulong,
            data: *const c_char,
            width: c_uint,
            height: c_uint,
        ) -> c_ulong;
        fn XCreatePixmapCursor(
            d: *mut c_void,
            source: c_ulong,
            mask: c_ulong,
            fg: *const Color,
            bg: *const Color,
            x: c_uint,
            y: c_uint,
        ) -> c_ulong;
        fn XFreePixmap(d: *mut c_void, pixmap: c_ulong) -> c_int;
        fn XFreeCursor(d: *mut c_void, cursor: c_ulong) -> c_int;
        fn XChangeActivePointerGrab(
            d: *mut c_void,
            mask: c_uint,
            cursor: c_ulong,
            time: c_ulong,
        ) -> c_int;
        fn XUngrabPointer(d: *mut c_void, time: c_ulong) -> c_int;
        fn XFlush(d: *mut c_void) -> c_int;
    }
    impl Editor {
        pub(in crate::editor) fn restore_host_focus(&self) {
            let Some((display, window)) = self.x11() else {
                return;
            };
            if !self.drag_has_focus() {
                return;
            }
            let (mut root, mut parent, mut count) = (0, 0, 0);
            let mut children = std::ptr::null_mut();
            // SAFETY: 生存中の子ウィンドウの親を取得する。子がフォーカスを持つ間は
            // 親も表示可能。Xlib所有の子一覧を解放し、RevertToParentで親へ戻す。
            unsafe {
                let ok = XQueryTree(
                    display,
                    window,
                    &mut root,
                    &mut parent,
                    &mut children,
                    &mut count,
                );
                if !children.is_null() {
                    XFree(children.cast());
                }
                if ok != 0 && parent != 0 {
                    XSetInputFocus(display, parent, 2, 0);
                    XFlush(display);
                }
            }
        }

        fn x11(&self) -> Option<(*mut c_void, c_ulong)> {
            let RawWindowHandle::Xlib(window) = self.window.window_handle().ok()?.as_raw() else {
                return None;
            };
            let RawDisplayHandle::Xlib(display) = self.window.display_handle().ok()?.as_raw()
            else {
                return None;
            };
            Some((display.display?.as_ptr(), window.window))
        }
        pub(in crate::editor) fn drag_has_focus(&self) -> bool {
            let Some((display, window)) = self.x11() else {
                return false;
            };
            let (mut focus, mut revert) = (0, 0);
            // SAFETY: 生存中の接続と書き込み可能な戻り値。グラブ由来の疑似FocusOutを区別する。
            unsafe {
                XGetInputFocus(display, &mut focus, &mut revert);
            }
            focus == window
        }
        pub(in crate::editor) fn drag_pointer_position(&self) -> Option<egui::Pos2> {
            let (display, window) = self.x11()?;
            let (mut root, mut child, mut rx, mut ry, mut x, mut y, mut mask) =
                (0, 0, 0, 0, 0, 0, 0);
            // SAFETY: WindowContextが所有する接続を描画スレッドで借用する。
            // XlibはbaseviewのXCBと共有する接続を内部で同期し、イベントはここで取り出さない。
            let valid = unsafe {
                XQueryPointer(
                    display, window, &mut root, &mut child, &mut rx, &mut ry, &mut x, &mut y,
                    &mut mask,
                )
            };
            (valid != 0 && mask & (1 << 8) != 0).then(|| egui::pos2(x as f32, y as f32))
        }
        pub(in crate::editor) fn hide_drag_pointer(&self) -> bool {
            let Some((display, window)) = self.x11() else {
                return false;
            };
            let color = Color {
                pixel: 0,
                red: 0,
                green: 0,
                blue: 0,
                flags: 0,
                pad: 0,
            };
            // SAFETY: 1x1の透明ビットマップと有効なXColorを同期的に読み取る。
            // 暗黙グラブのカーソルだけを変更し、全ボタン解放でサーバーが自動復旧する。
            unsafe {
                let bitmap = XCreateBitmapFromData(display, window, &0, 1, 1);
                if bitmap == 0 {
                    return false;
                }
                let cursor = XCreatePixmapCursor(display, bitmap, bitmap, &color, &color, 0, 0);
                XFreePixmap(display, bitmap);
                if cursor == 0 {
                    return false;
                }
                XChangeActivePointerGrab(display, (1 << 2) | (1 << 3) | (1 << 6), cursor, 0);
                // サーバーはグラブ終了までカーソルへの参照を保持する。
                XFreeCursor(display, cursor);
                XFlush(display);
            }
            true
        }
        pub(in crate::editor) fn warp_drag_pointer(&self, position: egui::Pos2) -> bool {
            let Some((display, window)) = self.x11() else {
                return false;
            };
            // SAFETY: 生存中の子ウィンドウ内の座標。表示接続の寿命はWindowContextに従う。
            unsafe {
                XWarpPointer(
                    display,
                    0,
                    window,
                    0,
                    0,
                    0,
                    0,
                    position.x as i32,
                    position.y as i32,
                );
                XFlush(display);
            }
            true
        }
        pub(in crate::editor) fn restore_drag_pointer(&self) {
            if let Some((display, _)) = self.x11() {
                // SAFETY: 同じ接続の自分のグラブだけを解除する。解放済みなら何もしない。
                unsafe {
                    XUngrabPointer(display, 0);
                    XFlush(display);
                }
            }
        }
    }
}

// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! パラメーターのマウス・数値入力とOpenGL描画を接続する。

pub(crate) mod edits;
mod pointer;
use edits::{Edit, Edits};

use crate::parameters::{self, FILTER_NAMES, NAMES, SharedParameters, WAVEFORM_NAMES};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, WindowContext, WindowEvent,
    WindowHandler,
};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers, NamedKey};
use std::{
    cell::{Cell, RefCell},
    sync::Arc,
    time::{Duration, Instant},
};

struct EditorState {
    /// 確定前の入力文字列。数値の解析・VST3値への換算は確定時に行う。
    text: [String; parameters::COUNT],
    /// 前フレームで入力欄が編集中だったか。フォーカス移動時にも入力を保持する。
    editing: [bool; parameters::COUNT],
    /// 文字列を編集した欄だけを確定し、ホスト値の表示更新を再通知しない。
    changed: [bool; parameters::COUNT],
    /// 現在操作しているノブ。終了時にtakeして二重終了を防ぐ。
    drag: Option<DragState>,
    /// 最後の描画で配置したノブの当たり判定。
    knobs: [egui::Rect; parameters::COUNT],
}

struct DragState {
    /// 操作対象のパラメーター。
    index: usize,
    /// 操作中の整数tick。ホストへの配送遅延で巻き戻さない。
    tick: u32,
    /// 1tick未満の上下移動量（物理px）。
    remainder: f32,
    /// 入力開始時のクライアント座標。
    anchor: egui::Pos2,
    /// 直接入力値を押下だけで丸めないための変更フラグ。
    changed: bool,
}

/// 上限での余分な移動を捨て、逆方向の操作を即座に受け付ける。
fn move_tick(tick: u32, remainder: f32, dy: f32, max: u32) -> (u32, f32) {
    let pixels = remainder - dy;
    let steps = (pixels / 4.0).trunc() as i32;
    let next = (i64::from(tick) + i64::from(steps)).clamp(0, i64::from(max)) as u32;
    let rest = if (next == 0 && pixels < 0.0) || (next == max && pixels > 0.0) {
        0.0
    } else {
        pixels - steps as f32 * 4.0
    };
    (next, rest)
}

fn max_tick(index: usize) -> u32 {
    if parameters::is_time(index) {
        371
    } else {
        match index {
            6 => 6,
            8 => 31,
            19 => 960,
            10 => 1200,
            13 => 600,
            11 => 950,
            _ => 100,
        }
    }
}

fn value_tick(index: usize, value: f64) -> u32 {
    let scale = if parameters::is_time(index) {
        parameters::time_scale()
    } else {
        f64::from(max_tick(index))
    };
    (value * scale).round() as u32
}

fn unit(index: usize) -> &'static str {
    if parameters::is_time(index) {
        "ms"
    } else {
        match index {
            13 => "dB",
            7 | 10 | 19 => "cent",
            _ => "",
        }
    }
}

pub(super) struct Editor {
    /// 描画スレッド内の数値入力状態。
    state: RefCell<EditorState>,
    /// 描画スレッド内だけで使う子ウィンドウの操作口。
    window: WindowContext,
    /// この子ウィンドウのOpenGLコンテキスト。
    gl: baseview::gl::GlContext,
    /// eguiのテクスチャ・描画資源。破棄時にもglを有効にする。
    painter: RefCell<egui_glow::Painter>,
    /// パラメーター行の配置と操作状態。
    context: egui::Context,
    /// 次フレームへ渡すマウスイベント。
    input: RefCell<egui::RawInput>,
    /// ボタンイベントへ付与する直近の物理ピクセル座標。
    pointer: Cell<egui::Pos2>,
    /// eguiへ渡す単調増加時刻の起点。
    start: Instant,
    /// Controllerと共有する表示値。
    parameters: Arc<SharedParameters>,
    /// 最後に描画した正規化値。ホスト更新時だけ再描画する。
    displayed: Cell<[f64; parameters::COUNT]>,
    /// ホストUIスレッドへの編集キュー。描画中にはホストを呼ばない。
    edits: Arc<Edits>,
    #[cfg(target_os = "windows")]
    /// Windowsでは同じUIスレッドのウィンドウタイマーから編集を配送する。
    view: std::rc::Weak<super::gui::EditorView>,
}

impl Editor {
    pub(super) fn new(
        window: WindowContext,
        parameters: Arc<SharedParameters>,
        edits: Arc<Edits>,
        #[cfg(target_os = "windows")] view: std::rc::Weak<super::gui::EditorView>,
    ) -> Result<Self, HandlerError> {
        let gl = window
            .gl_context()
            .ok_or_else(|| std::io::Error::other("OpenGL unavailable"))?;
        // SAFETY: baseviewの描画スレッドで、このウィンドウのGLだけを使用する。
        let painter = unsafe {
            gl.make_current()?;
            let context =
                egui_glow::glow::Context::from_loader_function_cstr(|s| gl.get_proc_address(s));
            let painter = egui_glow::Painter::new(Arc::new(context), "", None, false);
            gl.make_not_current()?;
            painter?
        };
        let editor = Self {
            state: RefCell::new(EditorState {
                text: std::array::from_fn(|_| String::new()),
                editing: [false; parameters::COUNT],
                changed: [false; parameters::COUNT],
                drag: None,
                knobs: [egui::Rect::NOTHING; parameters::COUNT],
            }),
            window,
            gl,
            painter: RefCell::new(painter),
            context: egui::Context::default(),
            input: RefCell::new(egui::RawInput::default()),
            pointer: Cell::new(egui::Pos2::ZERO),
            start: Instant::now(),
            parameters,
            displayed: Cell::new([f64::NAN; parameters::COUNT]),
            edits,
            #[cfg(target_os = "windows")]
            view,
        };
        editor.context.set_theme(egui::Theme::Light);
        editor.context.style_mut_of(egui::Theme::Light, |style| {
            for (text_style, font) in &mut style.text_styles {
                font.size = if *text_style == egui::TextStyle::Heading {
                    24.0
                } else {
                    18.0
                };
            }
        });
        // タイマーの寿命と解除はbaseviewの子ウィンドウに従う。
        editor.window.create_timer(Duration::from_millis(16))?;
        Ok(editor)
    }

    fn knob(&self, ui: &mut egui::Ui, index: usize, selected: f64) -> f64 {
        let (area, _) = ui.allocate_exact_size(egui::vec2(390.0, 21.0), egui::Sense::hover());
        let rect = egui::Rect::from_center_size(area.center(), egui::vec2(25.0, 25.0));
        let mut state = self.state.borrow_mut();
        state.knobs[index] = rect;
        let drag = state.drag.as_ref().filter(|d| d.index == index);
        let tick = drag.map_or_else(|| value_tick(index, selected), |d| d.tick);
        let value = if drag.is_some_and(|d| d.changed) {
            parameters::slider_to_plain(index, tick)
        } else {
            parameters::to_plain(index, selected)
        };
        let angle = std::f32::consts::PI * (0.75 + 1.5 * tick as f32 / max_tick(index) as f32);
        let center = rect.center();
        let color = if drag.is_some() {
            ui.visuals().selection.bg_fill
        } else {
            egui::Color32::from_gray(220)
        };
        ui.painter().circle(
            center,
            11.0,
            color,
            egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
        );
        ui.painter().line_segment(
            [center, center + egui::vec2(angle.cos(), angle.sin()) * 8.0],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(40, 90, 150)),
        );
        if drag.is_some() {
            let label = format!("{value:.3} {}", unit(index));
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("knob-value"),
            ));
            let text =
                painter.layout_no_wrap(label, egui::FontId::monospace(18.0), egui::Color32::BLACK);
            let bounds = egui::Rect::from_min_size(
                egui::pos2(
                    center.x - text.size().x / 2.0,
                    rect.top() - 6.0 - text.size().y,
                ),
                text.size(),
            );
            painter.rect_filled(
                bounds.expand(4.0),
                3.0,
                egui::Color32::from_rgb(255, 250, 220),
            );
            painter.galley(bounds.min, text, egui::Color32::BLACK);
        }
        value
    }

    fn begin_drag(&self, index: usize) {
        self.end_drag();
        let Some(anchor) = self.drag_pointer_position() else {
            return;
        };
        let mut selected = self.parameters.get(index);
        {
            let mut state = self.state.borrow_mut();
            for i in 0..parameters::COUNT {
                if state.editing[i] && state.changed[i] {
                    if let Some(value) = parameters::parse_numeric(i, &state.text[i]) {
                        self.edits.push(Edit::Set(i, value));
                        if i == index {
                            selected = value;
                        }
                    }
                    state.changed[i] = false;
                }
                state.editing[i] = false;
            }
        }
        self.context.memory_mut(|memory| {
            if let Some(id) = memory.focused() {
                memory.surrender_focus(id);
            }
        });
        if !self.edits.push(Edit::Begin(index)) {
            return;
        }
        self.state.borrow_mut().drag = Some(DragState {
            index,
            tick: value_tick(index, selected),
            remainder: 0.0,
            anchor,
            changed: false,
        });
        let _ = self.window.focus();
        if !self.hide_drag_pointer() {
            self.end_drag();
        }
        self.window.request_redraw();
    }

    fn move_drag(&self) {
        if !self.drag_has_focus() {
            self.end_drag();
            return;
        }
        let Some(position) = self.drag_pointer_position() else {
            self.end_drag();
            return;
        };
        let anchor = {
            let mut state = self.state.borrow_mut();
            let Some(drag) = state.drag.as_mut() else {
                return;
            };
            let (tick, rest) = move_tick(
                drag.tick,
                drag.remainder,
                position.y - drag.anchor.y,
                max_tick(drag.index),
            );
            drag.remainder = rest;
            if tick != drag.tick {
                drag.tick = tick;
                drag.changed = true;
                self.edits.push(Edit::Value(parameters::to_normalized(
                    drag.index,
                    parameters::slider_to_plain(drag.index, tick),
                )));
            }
            drag.anchor
        };
        // OSの現在位置を使うため、引き戻しの通知や溜まった移動イベントを二重計上しない。
        if position != anchor && !self.warp_drag_pointer(anchor) {
            self.end_drag();
        }
        self.window.request_redraw();
    }

    fn end_drag(&self) {
        let drag = self.state.borrow_mut().drag.take();
        if let Some(drag) = drag {
            self.restore_drag_pointer();
            self.edits.push(Edit::End(drag.changed.then(|| {
                parameters::to_normalized(
                    drag.index,
                    parameters::slider_to_plain(drag.index, drag.tick),
                )
            })));
            self.window.request_redraw();
        }
    }

    fn numeric_input(&self, ui: &mut egui::Ui, index: usize, value: f64, slider_edited: bool) {
        let mut state = self.state.borrow_mut();
        if slider_edited || !state.editing[index] {
            // 固定小数桁・右寄せで1の位を揃える。編集開始時には丸め前の値を渡す。
            state.text[index] = if matches!(index, 6 | 8) {
                format!("{value:.0}")
            } else {
                format!("{value:.3}")
            };
            state.changed[index] = false;
        }
        let cancel = state.editing[index] && ui.input(|i| i.key_pressed(egui::Key::Escape));
        let response = ui.add(
            egui::TextEdit::singleline(&mut state.text[index])
                .id_salt(("number", index))
                .font(egui::TextStyle::Monospace)
                .align(egui::Align2::RIGHT_CENTER)
                .margin(egui::vec2(4.0, 0.0))
                .desired_width(125.0),
        );
        if response.gained_focus() {
            state.text[index] = value.to_string();
            let _ = self.window.focus();
        }
        state.changed[index] |= response.changed();
        if response.lost_focus() && state.changed[index] && !cancel {
            if let Some(normalized) = parameters::parse_numeric(index, &state.text[index]) {
                let _ = self.edits.push(Edit::Set(index, normalized));
            }
            state.changed[index] = false;
        }
        state.editing[index] = response.has_focus();
        if cancel {
            state.changed[index] = false;
            state.editing[index] = false;
            response.surrender_focus();
        }
    }

    fn keyboard_input(&self, event: KeyboardEvent) -> EventStatus {
        if event.state == KeyState::Down
            && event.key == Key::Named(NamedKey::Escape)
            && self.state.borrow().drag.is_some()
        {
            self.end_drag();
            return EventStatus::Captured;
        }
        if !self.context.egui_wants_keyboard_input() || event.is_composing {
            return EventStatus::Ignored;
        }
        let modifiers = egui::Modifiers {
            alt: event.modifiers.contains(Modifiers::ALT),
            ctrl: event.modifiers.contains(Modifiers::CONTROL),
            shift: event.modifiers.contains(Modifiers::SHIFT),
            command: event.modifiers.contains(Modifiers::CONTROL),
            mac_cmd: false,
        };
        let key = match &event.key {
            Key::Named(NamedKey::Enter) => Some(egui::Key::Enter),
            Key::Named(NamedKey::Escape) => Some(egui::Key::Escape),
            Key::Named(NamedKey::Tab) => Some(egui::Key::Tab),
            Key::Named(NamedKey::Backspace) => Some(egui::Key::Backspace),
            Key::Named(NamedKey::Delete) => Some(egui::Key::Delete),
            Key::Named(NamedKey::ArrowLeft) => Some(egui::Key::ArrowLeft),
            Key::Named(NamedKey::ArrowRight) => Some(egui::Key::ArrowRight),
            Key::Named(NamedKey::ArrowUp) => Some(egui::Key::ArrowUp),
            Key::Named(NamedKey::ArrowDown) => Some(egui::Key::ArrowDown),
            Key::Named(NamedKey::Home) => Some(egui::Key::Home),
            Key::Named(NamedKey::End) => Some(egui::Key::End),
            Key::Character(text) if text.eq_ignore_ascii_case("a") => Some(egui::Key::A),
            _ => None,
        };
        let mut input = self.input.borrow_mut();
        input.events.push(egui::Event::ModifiersChanged(modifiers));
        let pressed = event.state == KeyState::Down;
        if let Some(key) = key {
            input.events.push(egui::Event::Key {
                key,
                physical_key: None,
                pressed,
                repeat: event.repeat,
                modifiers,
            });
        }
        if pressed
            && !modifiers.ctrl
            && !modifiers.alt
            && let Key::Character(text) = event.key
        {
            input.events.push(egui::Event::Text(text));
        }
        self.window.request_redraw();
        EventStatus::Captured
    }
}

impl WindowHandler for Editor {
    fn draw(&self) -> Result<(), HandlerError> {
        let size = self.window.size().physical;
        let mut input = self.input.borrow_mut().take();
        input.time = Some(self.start.elapsed().as_secs_f64());
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size.width as f32, size.height as f32),
        ));
        input.max_texture_side = Some(self.painter.borrow().max_texture_side());
        let selected = self.parameters.snapshot();
        self.displayed.set(selected);
        let mut output = self.context.run_ui(input, |ui| {
            egui::Frame::new()
                .inner_margin(12)
                .fill(ui.visuals().panel_fill)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.add_space(14.0);
                    ui.heading("Sample VSTi");
                    ui.add_space(12.0);
                    egui::Grid::new("parameters")
                        .num_columns(3)
                        .min_col_width(160.0)
                        .spacing([16.0, 8.0])
                        .show(ui, |ui| {
                            ui.label("Waveform");
                            let mut index = parameters::to_plain(0, selected[0]) as usize;
                            if egui::ComboBox::from_id_salt("waveform")
                                .width(200.0)
                                .show_index(ui, &mut index, WAVEFORM_NAMES.len(), |i| {
                                    WAVEFORM_NAMES[i]
                                })
                                .changed()
                            {
                                let _ = self.edits.push(Edit::Set(0, index as f64 / 3.0));
                            }
                            ui.label("");
                            ui.end_row();

                            for i in 1..NAMES.len() {
                                ui.label(NAMES[i]);
                                if matches!(i, 9 | 12) {
                                    let names: &[&str] = if i == 9 {
                                        &FILTER_NAMES
                                    } else {
                                        &parameters::PHASE_NAMES
                                    };
                                    let mut index = parameters::to_plain(i, selected[i]) as usize;
                                    if egui::ComboBox::from_id_salt(if i == 9 {
                                        "filter"
                                    } else {
                                        "phase"
                                    })
                                    .width(200.0)
                                    .show_index(ui, &mut index, names.len(), |n| names[n])
                                    .changed()
                                    {
                                        let _ = self.edits.push(Edit::Set(
                                            i,
                                            parameters::to_normalized(i, index as f64),
                                        ));
                                    }
                                    ui.label("");
                                    ui.end_row();
                                    continue;
                                }
                                let value = self.knob(ui, i, selected[i]);
                                ui.horizontal(|ui| {
                                    let dragging = self
                                        .state
                                        .borrow()
                                        .drag
                                        .as_ref()
                                        .is_some_and(|d| d.index == i && d.changed);
                                    self.numeric_input(ui, i, value, dragging);
                                    ui.label(unit(i));
                                });
                                ui.end_row();
                            }
                        });
                });
        });
        let shapes = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        // SAFETY: GL処理をこの描画呼び出し内に限定し、終了時にコンテキストを解除する。
        unsafe {
            self.gl.make_current()?;
        }
        {
            let mut painter = self.painter.borrow_mut();
            painter.clear([size.width, size.height], [0.0, 0.0, 0.0, 1.0]);
            painter.paint_and_update_textures(
                [size.width, size.height],
                output.pixels_per_point,
                &shapes,
                &mut output.textures_delta,
            );
        }
        let result = self.gl.swap_buffers();
        // SAFETY: 上で有効にした同じコンテキスト。
        unsafe {
            self.gl.make_not_current()?;
        }
        result?;
        if self.context.has_requested_repaint() {
            self.window.request_redraw();
        }
        Ok(())
    }

    fn resized(&self, _: baseview::WindowSize) -> Result<(), HandlerError> {
        Ok(())
    }

    fn on_event(&self, event: Event) -> EventStatus {
        let event = match event {
            Event::Keyboard(event) => return self.keyboard_input(event),
            Event::Window(WindowEvent::Focused | WindowEvent::Unfocused) => {
                let focused = matches!(event, Event::Window(WindowEvent::Focused))
                    || (self.state.borrow().drag.is_some() && self.drag_has_focus());
                if !focused {
                    self.end_drag();
                }
                self.input.borrow_mut().focused = focused;
                egui::Event::WindowFocused(focused)
            }
            Event::Window(WindowEvent::WillClose) => {
                self.end_drag();
                return EventStatus::Captured;
            }
            Event::Mouse(MouseEvent::CursorMoved { position, .. }) => {
                if self.state.borrow().drag.is_some() {
                    self.move_drag();
                    return EventStatus::Captured;
                }
                let pos = egui::pos2(position.x as f32, position.y as f32);
                self.pointer.set(pos);
                egui::Event::PointerMoved(pos)
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => {
                let index = self
                    .state
                    .borrow()
                    .knobs
                    .iter()
                    .position(|rect| rect.contains(self.pointer.get()));
                if let Some(index) = index.filter(|_| !egui::Popup::is_any_open(&self.context)) {
                    self.begin_drag(index);
                    return EventStatus::Captured;
                }
                egui::Event::PointerButton {
                    pos: self.pointer.get(),
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: egui::Modifiers::NONE,
                }
            }
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => {
                let anchor = self.state.borrow().drag.as_ref().map(|d| d.anchor);
                if let Some(anchor) = anchor {
                    self.warp_drag_pointer(anchor);
                    self.end_drag();
                    return EventStatus::Captured;
                }
                egui::Event::PointerButton {
                    pos: self.pointer.get(),
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: egui::Modifiers::NONE,
                }
            }
            Event::Mouse(MouseEvent::CursorLeft) => {
                if !self.drag_has_focus() || self.drag_pointer_position().is_none() {
                    self.end_drag();
                }
                egui::Event::PointerGone
            }
            _ => return EventStatus::Ignored,
        };
        self.input.borrow_mut().events.push(event);
        self.window.request_redraw();
        EventStatus::Captured
    }

    fn on_timer(&self, _: &baseview::TimerHandle) {
        if self.displayed.get() != self.parameters.snapshot() {
            self.window.request_redraw();
        }
        #[cfg(target_os = "windows")]
        if let Some(view) = self.view.upgrade() {
            // 描画の借用やGL操作が終了した後、ホストのUIスレッドから通知する。
            view.poll();
        }
    }
}

impl Drop for Editor {
    fn drop(&mut self) {
        self.end_drag();
        // SAFETY: baseviewは描画スレッドでhandlerを破棄する。GL資源も同じ場所で解放する。
        unsafe {
            if self.gl.make_current().is_ok() {
                self.painter.get_mut().destroy();
                let _ = self.gl.make_not_current();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn every_knob_tick_is_reachable_in_both_directions() {
        for index in 1..parameters::COUNT {
            if matches!(index, 9 | 12) {
                continue;
            }
            let max = max_tick(index);
            for tick in 0..max {
                let (same, remainder) = move_tick(tick, 0.0, -3.0, max);
                assert_eq!(same, tick);
                assert_eq!(move_tick(same, remainder, -1.0, max), (tick + 1, 0.0));
                assert_eq!(move_tick(tick + 1, 0.0, 4.0, max), (tick, 0.0));
            }
            assert_eq!(move_tick(max, 0.0, -100.0, max), (max, 0.0));
            assert_eq!(move_tick(max, 0.0, 4.0, max), (max - 1, 0.0));
            assert_eq!(move_tick(0, 0.0, 100.0, max), (0, 0.0));
        }
    }
}

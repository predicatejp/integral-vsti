// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! カテゴリ別のノブ・選択リストとOpenGL描画を接続する。

pub(crate) mod edits;
mod pointer;
use edits::{Edit, Edits};

use crate::parameters::{self, NAMES, SharedParameters};
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

/// ノブの表示名。ホスト公開名とオーバーレイはparameters::NAMESを使う。
const KNOB_NAMES: [&str; parameters::COUNT] = [
    "Waveform", "A", "H", "D", "S", "R", "Uni", "Det", "Poly", "Filter", "F", "Res", "Phase",
    "Vol", "A", "H", "D", "S", "R", "Amt", "Width", "Pan", "Vol", "Waveform", "Tune", "Vol",
    "Waveform", "Tune", "Vol",
];

/// グループの寸法とノブの並ぶ方向。
struct GroupLayout {
    /// 枠内の最小サイズ。
    size: egui::Vec2,
    /// Masterではノブを上から下へ並べる。
    vertical_knobs: bool,
}

/// 全パラメーターで共通のオーバーレイ寸法（物理px）。
const OVERLAY_SIZE: egui::Vec2 = egui::vec2(224.0, 60.0);

fn overlay_rect(knob: egui::Rect, viewport: egui::Rect) -> egui::Rect {
    let min = egui::pos2(
        knob.center().x - OVERLAY_SIZE.x / 2.0,
        knob.top() - 6.0 - OVERLAY_SIZE.y,
    );
    egui::Rect::from_min_size(
        egui::pos2(
            min.x
                .clamp(viewport.left(), viewport.right() - OVERLAY_SIZE.x),
            min.y
                .clamp(viewport.top(), viewport.bottom() - OVERLAY_SIZE.y),
        ),
        OVERLAY_SIZE,
    )
}

struct EditorState {
    /// 現在操作しているノブ。終了時にtakeして二重終了を防ぐ。
    drag: Option<DragState>,
    /// 最後の描画で配置したノブの当たり判定。
    knobs: [egui::Rect; parameters::COUNT],
}

struct DragState {
    #[cfg(target_os = "windows")]
    /// 操作開始時のキー入力先と前面ウィンドウ。所有せず変化の検知だけに使う。
    keyboard_focus: [usize; 2],
    /// 操作対象のパラメーター。
    index: usize,
    /// 操作中の整数tick。ホストへの配送遅延で巻き戻さない。
    tick: u32,
    /// 1tick未満の上下移動量（物理px）。
    remainder: f32,
    /// 入力開始時のクライアント座標。
    anchor: egui::Pos2,
    /// ホストからの連続値を押下だけで丸めないための変更フラグ。
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

pub(super) struct Editor {
    /// 描画スレッド内のノブ操作状態。
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

    fn knob(&self, ui: &mut egui::Ui, index: usize, selected: f64, width: f32) {
        let (area, _) = ui.allocate_exact_size(egui::vec2(width, 50.0), egui::Sense::hover());
        let rect = egui::Rect::from_center_size(area.center(), egui::vec2(50.0, 50.0));
        let mut state = self.state.borrow_mut();
        state.knobs[index] = rect;
        let drag = state.drag.as_ref().filter(|d| d.index == index);
        let tick = drag.map_or_else(|| parameters::to_tick(index, selected), |d| d.tick);
        let value = if drag.is_some_and(|d| d.changed) {
            parameters::tick_to_plain(index, tick)
        } else {
            parameters::to_plain(index, selected)
        };
        let angle =
            std::f32::consts::PI * (0.75 + 1.5 * tick as f32 / parameters::max_tick(index) as f32);
        let center = rect.center();
        let color = if drag.is_some() {
            ui.visuals().selection.bg_fill
        } else {
            egui::Color32::from_gray(220)
        };
        ui.painter().circle(
            center,
            22.0,
            color,
            egui::Stroke::new(1.0, egui::Color32::from_gray(100)),
        );
        ui.painter().line_segment(
            [center, center + egui::vec2(angle.cos(), angle.sin()) * 16.0],
            egui::Stroke::new(2.0, egui::Color32::from_rgb(40, 90, 150)),
        );
        if drag.is_some() {
            let value_label = if parameters::is_volume(index) && value == f64::NEG_INFINITY {
                "−∞ dB".to_owned()
            } else if matches!(index, 6 | 8) {
                format!("{value:.0}")
            } else {
                format!("{value:.3} {}", parameters::unit(index))
            };
            let label = format!("{}\n{}", NAMES[index], value_label.trim_end());
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("knob-value"),
            ));
            let mut layout = egui::text::LayoutJob::simple(
                label,
                egui::FontId::monospace(18.0),
                egui::Color32::BLACK,
                f32::INFINITY,
            );
            layout.halign = egui::Align::Center;
            let text = painter.layout_job(layout);
            let bounds = overlay_rect(rect, ui.ctx().viewport_rect());
            painter.rect_filled(bounds, 3.0, egui::Color32::from_rgb(255, 250, 220));
            painter.galley(
                egui::pos2(bounds.center().x, bounds.center().y - text.size().y / 2.0),
                text,
                egui::Color32::BLACK,
            );
        }
    }

    fn begin_drag(&self, index: usize) {
        self.end_drag();
        let Some(anchor) = self.drag_pointer_position() else {
            return;
        };
        let selected = self.parameters.get(index);
        if !self.edits.push(Edit::Begin(index)) {
            return;
        }
        self.state.borrow_mut().drag = Some(DragState {
            #[cfg(target_os = "windows")]
            keyboard_focus: self.keyboard_focus(),
            index,
            tick: parameters::to_tick(index, selected),
            remainder: 0.0,
            anchor,
            changed: false,
        });
        if !self.begin_drag_input() {
            self.end_drag();
        }
        self.window.request_redraw();
    }

    fn move_drag(&self) {
        let Some(position) = self.current_drag_position() else {
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
                parameters::max_tick(drag.index),
            );
            drag.remainder = rest;
            if tick != drag.tick {
                drag.tick = tick;
                drag.changed = true;
                self.edits.push(Edit::Value(parameters::to_normalized(
                    drag.index,
                    parameters::tick_to_plain(drag.index, tick),
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
            self.end_drag_input();
            self.edits.push(Edit::End(drag.changed.then(|| {
                parameters::to_normalized(
                    drag.index,
                    parameters::tick_to_plain(drag.index, drag.tick),
                )
            })));
            self.window.request_redraw();
        }
    }

    fn choice(&self, ui: &mut egui::Ui, index: usize, selected: f64) {
        let names = parameters::choices(index);
        let mut choice = parameters::to_plain(index, selected) as usize;
        if egui::ComboBox::from_id_salt(index)
            .width(130.0)
            .show_index(ui, &mut choice, names.len(), |i| names[i])
            .changed()
        {
            self.edits.push(Edit::Set(
                index,
                parameters::to_normalized(index, choice as f64),
            ));
        }
    }

    fn group(
        &self,
        ui: &mut egui::Ui,
        title: &str,
        choices: &[usize],
        knobs: &[usize],
        layout: GroupLayout,
        selected: &[f64; parameters::COUNT],
    ) {
        egui::Frame::group(ui.style())
            .inner_margin(8)
            .show(ui, |ui| {
                ui.vertical(|ui| {
                    let knob_count = if layout.vertical_knobs {
                        1
                    } else {
                        knobs.len()
                    };
                    let knob_width = knob_count as f32 * 60.0 - 8.0;
                    let choice_width = choices.len() as f32 * 138.0 - 8.0;
                    ui.set_min_size(layout.size);
                    ui.set_width(layout.size.x.max(knob_width).max(choice_width));
                    if choices.len() == 1 && parameters::is_waveform(choices[0]) {
                        ui.horizontal(|ui| {
                            ui.heading(title);
                            self.choice(ui, choices[0], selected[choices[0]]);
                        });
                    } else {
                        ui.heading(title);
                        if !choices.is_empty() {
                            ui.horizontal(|ui| {
                                for &index in choices {
                                    ui.vertical(|ui| {
                                        ui.label(KNOB_NAMES[index]);
                                        self.choice(ui, index, selected[index]);
                                    });
                                }
                            });
                        }
                    }
                    ui.add_space(6.0);
                    let knob_layout = if layout.vertical_knobs {
                        egui::Layout::top_down(egui::Align::Center)
                    } else {
                        egui::Layout::left_to_right(egui::Align::Min)
                    };
                    ui.with_layout(knob_layout, |ui| {
                        for &index in knobs {
                            ui.allocate_ui_with_layout(
                                egui::vec2(52.0, 74.0),
                                egui::Layout::top_down(egui::Align::Center),
                                |ui| {
                                    self.knob(ui, index, selected[index], 52.0);
                                    ui.label(KNOB_NAMES[index]);
                                },
                            );
                        }
                    });
                });
            });
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
            Key::Named(NamedKey::ArrowLeft) => Some(egui::Key::ArrowLeft),
            Key::Named(NamedKey::ArrowRight) => Some(egui::Key::ArrowRight),
            Key::Named(NamedKey::ArrowUp) => Some(egui::Key::ArrowUp),
            Key::Named(NamedKey::ArrowDown) => Some(egui::Key::ArrowDown),
            Key::Named(NamedKey::Home) => Some(egui::Key::Home),
            Key::Named(NamedKey::End) => Some(egui::Key::End),
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
                .inner_margin(8)
                .fill(ui.visuals().panel_fill)
                .show(ui, |ui| {
                    ui.set_min_size(ui.available_size());
                    ui.heading("Sample VSTi");
                    ui.add_space(4.0);
                    ui.horizontal_top(|ui| {
                        ui.vertical(|ui| {
                            self.group(
                                ui,
                                "Osc1",
                                &[0],
                                &[6, 7, 20, 22],
                                GroupLayout {
                                    size: egui::vec2(292.0, 0.0),
                                    vertical_knobs: false,
                                },
                                &selected,
                            );
                            for (title, wave, tune, volume) in
                                [("Osc2", 23, 24, 25), ("Osc3", 26, 27, 28)]
                            {
                                ui.add_space(5.0);
                                self.group(
                                    ui,
                                    title,
                                    &[wave],
                                    &[tune, volume],
                                    GroupLayout {
                                        size: egui::vec2(292.0, 0.0),
                                        vertical_knobs: false,
                                    },
                                    &selected,
                                );
                            }
                            ui.add_space(5.0);
                            self.group(
                                ui,
                                "Amplitude envelope",
                                &[],
                                &[1, 2, 3, 4, 5],
                                GroupLayout {
                                    size: egui::vec2(292.0, 0.0),
                                    vertical_knobs: false,
                                },
                                &selected,
                            );
                        });
                        ui.vertical(|ui| {
                            self.group(
                                ui,
                                "Filter",
                                &[9],
                                &[10, 11],
                                GroupLayout {
                                    size: egui::vec2(352.0, 162.0),
                                    vertical_knobs: false,
                                },
                                &selected,
                            );
                            ui.add_space(5.0);
                            self.group(
                                ui,
                                "Cutoff envelope",
                                &[],
                                &[14, 15, 16, 17, 18, 19],
                                GroupLayout {
                                    size: egui::vec2(352.0, 0.0),
                                    vertical_knobs: false,
                                },
                                &selected,
                            );
                        });
                        self.group(
                            ui,
                            "Master",
                            &[12],
                            &[8, 13, 21],
                            GroupLayout {
                                size: egui::vec2(172.0, 0.0),
                                vertical_knobs: true,
                            },
                            &selected,
                        );
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
                if self.current_drag_position().is_none() {
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
        // キーボードフォーカスを取得しないWindowsでは、終了条件もタイマーで確認する。
        #[cfg(target_os = "windows")]
        if self.state.borrow().drag.is_some() && self.current_drag_position().is_none() {
            self.end_drag();
        }
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
    fn overlay_stays_inside_viewport_at_every_edge() {
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1024.0, 768.0));
        for x in [0.0, 43.0, 512.0, 1024.0] {
            for y in [0.0, 114.0, 384.0, 768.0] {
                let knob = egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(50.0, 50.0));
                let bounds = overlay_rect(knob, viewport);
                assert_eq!(bounds.size(), egui::vec2(224.0, 60.0));
                assert!(viewport.contains_rect(bounds));
                if (112.0..=912.0).contains(&x) {
                    assert_eq!(bounds.center().x, x);
                }
                if y >= 91.0 {
                    assert_eq!(bounds.bottom(), knob.top() - 6.0);
                }
            }
        }
    }

    #[test]
    fn every_knob_tick_is_reachable_in_both_directions() {
        for index in 1..parameters::COUNT {
            if !parameters::choices(index).is_empty() {
                continue;
            }
            let max = parameters::max_tick(index);
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

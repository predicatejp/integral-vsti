// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! パラメーターのマウス・数値入力とOpenGL描画を接続する。

use crate::parameters::{self, FILTER_NAMES, NAMES, SharedParameters, WAVEFORM_NAMES};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, WindowContext, WindowEvent,
    WindowHandler,
};
use keyboard_types::{Key, KeyState, KeyboardEvent, Modifiers, NamedKey};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, mpsc::SyncSender},
    time::{Duration, Instant},
};

struct EditorState {
    /// 確定前の入力文字列。数値の解析・VST3値への換算は確定時に行う。
    text: [String; parameters::COUNT],
    /// 前フレームで入力欄が編集中だったか。フォーカス移動時にも入力を保持する。
    editing: [bool; parameters::COUNT],
    /// 文字列を編集した欄だけを確定し、ホスト値の表示更新を再通知しない。
    changed: [bool; parameters::COUNT],
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
    edits: SyncSender<(usize, f64)>,
    #[cfg(target_os = "windows")]
    /// Windowsでは同じUIスレッドのウィンドウタイマーから編集を配送する。
    view: std::rc::Weak<super::gui::EditorView>,
}

impl Editor {
    pub(super) fn new(
        window: WindowContext,
        parameters: Arc<SharedParameters>,
        edits: SyncSender<(usize, f64)>,
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
                let _ = self.edits.try_send((index, normalized));
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
                        .min_col_width(80.0)
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
                                let _ = self.edits.try_send((0, index as f64 / 3.0));
                            }
                            ui.label("");
                            ui.end_row();
                            ui.spacing_mut().slider_width = 390.0;
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
                                        let _ = self.edits.try_send((
                                            i,
                                            parameters::to_normalized(i, index as f64),
                                        ));
                                    }
                                    ui.label("");
                                    ui.end_row();
                                    continue;
                                }
                                let time = parameters::is_time(i);
                                let max: u32 = if time {
                                    371
                                } else if i == 6 {
                                    6
                                } else if i == 8 {
                                    31
                                } else if i == 10 {
                                    1200
                                } else if i == 13 {
                                    600
                                } else if i == 11 {
                                    950
                                } else {
                                    100
                                };
                                let scale = if time {
                                    parameters::time_scale()
                                } else {
                                    f64::from(max)
                                };
                                let mut tick = (selected[i] * scale).round() as u32;
                                let response = ui.add(
                                    egui::Slider::new(&mut tick, 0..=max)
                                        // 丸い数への補正で隣のtickが選べなくなるのを防ぐ。
                                        .smart_aim(false)
                                        .show_value(false),
                                );
                                // ホスト値から表示用の整数に丸めただけでは再通知しない。
                                let edited =
                                    response.changed() && response.interact_pointer_pos().is_some();
                                let value = if edited {
                                    let value = parameters::slider_to_plain(i, tick);
                                    let _ = self
                                        .edits
                                        .try_send((i, parameters::to_normalized(i, value)));
                                    value
                                } else {
                                    parameters::to_plain(i, selected[i])
                                };
                                ui.horizontal(|ui| {
                                    self.numeric_input(ui, i, value, edited);
                                    ui.label(if time {
                                        "ms"
                                    } else if i == 13 {
                                        "dB"
                                    } else if matches!(i, 7 | 10) {
                                        "cent"
                                    } else {
                                        ""
                                    });
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
                let focused = matches!(event, Event::Window(WindowEvent::Focused));
                self.input.borrow_mut().focused = focused;
                egui::Event::WindowFocused(focused)
            }
            Event::Mouse(MouseEvent::CursorMoved { position, .. }) => {
                let pos = egui::pos2(position.x as f32, position.y as f32);
                self.pointer.set(pos);
                egui::Event::PointerMoved(pos)
            }
            Event::Mouse(MouseEvent::ButtonPressed {
                button: MouseButton::Left,
                ..
            }) => egui::Event::PointerButton {
                pos: self.pointer.get(),
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            },
            Event::Mouse(MouseEvent::ButtonReleased {
                button: MouseButton::Left,
                ..
            }) => egui::Event::PointerButton {
                pos: self.pointer.get(),
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            },
            Event::Mouse(MouseEvent::CursorLeft) => egui::Event::PointerGone,
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
        // SAFETY: baseviewは描画スレッドでhandlerを破棄する。GL資源も同じ場所で解放する。
        unsafe {
            if self.gl.make_current().is_ok() {
                self.painter.get_mut().destroy();
                let _ = self.gl.make_not_current();
            }
        }
    }
}

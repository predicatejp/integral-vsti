// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 波形選択・音量エンベロープに必要なマウス入力とOpenGL描画だけを接続する。

use crate::parameters::{self, FILTER_NAMES, NAMES, SharedParameters, WAVEFORM_NAMES};
use baseview::{
    Event, EventStatus, HandlerError, MouseButton, MouseEvent, WindowContext, WindowHandler,
};
use std::{
    cell::{Cell, RefCell},
    sync::{Arc, mpsc::SyncSender},
    time::{Duration, Instant},
};

pub(super) struct Editor {
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
    displayed: Cell<[f64; 12]>,
    /// ホストUIスレッドへの編集キュー。描画中にはホストを呼ばない。
    edits: SyncSender<(usize, f64)>,
    #[cfg(target_os = "windows")]
    /// Windowsでは同じUIスレッドのウィンドウタイマーから編集を配送する。
    view: std::rc::Weak<super::gui::ViewState>,
}

impl Editor {
    pub(super) fn new(
        window: WindowContext,
        parameters: Arc<SharedParameters>,
        edits: SyncSender<(usize, f64)>,
        #[cfg(target_os = "windows")] view: std::rc::Weak<super::gui::ViewState>,
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
            window,
            gl,
            painter: RefCell::new(painter),
            context: egui::Context::default(),
            input: RefCell::new(egui::RawInput::default()),
            pointer: Cell::new(egui::Pos2::ZERO),
            start: Instant::now(),
            parameters,
            displayed: Cell::new([f64::NAN; 12]),
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
                                if i == 9 {
                                    let mut index = parameters::to_plain(i, selected[i]) as usize;
                                    if egui::ComboBox::from_id_salt("filter")
                                        .width(200.0)
                                        .show_index(ui, &mut index, FILTER_NAMES.len(), |n| {
                                            FILTER_NAMES[n]
                                        })
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
                                // 等幅・固定桁数で整数部を右寄せし、1の位と小数点を揃える。
                                ui.monospace(if time {
                                    format!("{value:7.2} ms")
                                } else if matches!(i, 6 | 8) {
                                    format!("{value:8.0}")
                                } else if i == 10 {
                                    format!("{value:+5.0} cent")
                                } else if i == 7 {
                                    format!("{value:5.1} cent")
                                } else {
                                    format!("{value:8.3}")
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

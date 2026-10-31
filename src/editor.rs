// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! カテゴリ別のノブ・ドロップダウンとOpenGL描画を接続する。

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
    "Waveform", "Tune", "Vol", "FM", "FM",
];

// 操作盤・選択欄・ノブ・オーバーレイで共通の配色。
const PANEL: egui::Color32 = egui::Color32::from_rgb(248, 249, 245);
const INK: egui::Color32 = egui::Color32::from_rgb(23, 43, 38);
const MUTED: egui::Color32 = egui::Color32::from_rgb(53, 71, 62);
const LINE: egui::Color32 = egui::Color32::from_rgb(143, 160, 147);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(20, 96, 76);

/// 枠内余白・見出しと本文の間隔・行間・グループ間隔。
const GAP: f32 = 8.0;
/// 全グループで共通の見出し／選択欄の高さ。
const HEADER_HEIGHT: f32 = 28.0;
/// ノブとラベルの高さ。横幅は各行で均等に割り当てる。
const KNOB_CELL: egui::Vec2 = egui::vec2(64.0, 80.0);

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
            style.spacing.button_padding = egui::vec2(10.0, 3.0);
            style.animation_time = 0.0;
            style.visuals.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, INK);
            style.visuals.panel_fill = egui::Color32::from_rgb(232, 237, 231);
            style.visuals.window_fill = PANEL;
            style.visuals.selection.bg_fill = ACCENT;
            style.visuals.selection.stroke = egui::Stroke::new(1.0, egui::Color32::WHITE);
            for widget in [
                &mut style.visuals.widgets.inactive,
                &mut style.visuals.widgets.hovered,
                &mut style.visuals.widgets.active,
                &mut style.visuals.widgets.open,
            ] {
                widget.corner_radius = egui::CornerRadius::same(4);
                widget.bg_stroke = egui::Stroke::new(1.0, LINE);
                widget.fg_stroke = egui::Stroke::new(1.0, INK);
            }
            style.visuals.widgets.inactive.weak_bg_fill = egui::Color32::from_rgb(235, 241, 234);
            style.visuals.widgets.hovered.weak_bg_fill = egui::Color32::from_rgb(219, 232, 222);
            style.visuals.widgets.active.weak_bg_fill = egui::Color32::from_rgb(207, 225, 215);
        });
        // タイマーの寿命と解除はbaseviewの子ウィンドウに従う。
        editor.window.create_timer(Duration::from_millis(16))?;
        Ok(editor)
    }

    fn knob(&self, ui: &mut egui::Ui, index: usize, selected: f64, width: f32) {
        let (area, _) =
            ui.allocate_exact_size(egui::vec2(width, KNOB_CELL.y), egui::Sense::hover());
        let rect = egui::Rect::from_center_size(
            egui::pos2(area.center().x, area.top() + 30.0),
            egui::vec2(50.0, 50.0),
        );
        let mut state = self.state.borrow_mut();
        state.knobs[index] = rect;
        let drag = state.drag.as_ref().filter(|d| d.index == index);
        let tick = drag.map_or_else(|| parameters::to_tick(index, selected), |d| d.tick);
        let value = if drag.is_some_and(|d| d.changed) {
            parameters::tick_to_plain(index, tick)
        } else {
            parameters::to_plain(index, selected)
        };
        let position = tick as f32 / parameters::max_tick(index) as f32;
        let angle = |t: f32| std::f32::consts::PI * (0.75 + 1.5 * t);
        let direction = |t: f32| egui::vec2(angle(t).cos(), angle(t).sin());
        let center = rect.center();
        let painter = ui.painter().clone();
        // 一定の目盛りと値の円弧。中点を持つパラメーターはゼロから両方向へ伸ばす。
        let zero = if matches!(index, 19 | 21 | 24 | 27) {
            0.5
        } else {
            0.0
        };
        for k in 0..=8 {
            let d = direction(k as f32 / 8.0);
            painter.line_segment(
                [center + d * 28.0, center + d * 30.0],
                egui::Stroke::new(1.0, LINE),
            );
        }
        let arc = |from: f32, to: f32| {
            (0..=32)
                .map(|k| center + direction(from + (to - from) * k as f32 / 32.0) * 26.0)
                .collect::<Vec<_>>()
        };
        painter.add(egui::Shape::line(
            arc(0.0, 1.0),
            egui::Stroke::new(2.0, LINE),
        ));
        painter.add(egui::Shape::line(
            arc(zero, position),
            egui::Stroke::new(3.0, ACCENT),
        ));
        painter.circle_filled(center + egui::vec2(0.0, 1.5), 22.0, LINE);
        painter.circle(
            center,
            22.0,
            egui::Color32::WHITE,
            egui::Stroke::new(1.0, if drag.is_some() { ACCENT } else { LINE }),
        );
        let d = direction(position);
        painter.line_segment(
            [center + d * 10.0, center + d * 18.0],
            egui::Stroke::new(3.0, ACCENT),
        );
        painter.text(
            egui::pos2(center.x, area.top() + 68.0),
            egui::Align2::CENTER_CENTER,
            KNOB_NAMES[index],
            egui::FontId::proportional(18.0),
            MUTED,
        );
        if drag.is_some() {
            let value_label = if parameters::is_volume(index) && value == f64::NEG_INFINITY {
                "−∞ dB".to_owned()
            } else {
                let number = format!("{value:.3}");
                format!(
                    "{} {}",
                    number.trim_end_matches('0').trim_end_matches('.'),
                    parameters::unit(index)
                )
            };
            let label = format!("{}\n{}", NAMES[index], value_label.trim_end());
            let painter = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Tooltip,
                egui::Id::unique("knob-value"),
            ));
            let mut layout = egui::text::LayoutJob::simple(
                label,
                egui::FontId::proportional(18.0),
                egui::Color32::WHITE,
                f32::INFINITY,
            );
            layout.halign = egui::Align::Center;
            let text = painter.layout_job(layout);
            let bounds = overlay_rect(rect, ui.ctx().viewport_rect());
            painter.rect_filled(bounds, 5.0, INK);
            painter.galley(
                egui::pos2(bounds.center().x, bounds.center().y - text.size().y / 2.0),
                text,
                egui::Color32::WHITE,
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
            .width(ui.available_width())
            .height(320.0)
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
        rows: &[&[usize]],
        rect: egui::Rect,
        selected: &[f64; parameters::COUNT],
    ) {
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 8.0, PANEL);
        painter.rect_stroke(
            rect,
            8.0,
            egui::Stroke::new(1.0, LINE),
            egui::StrokeKind::Inside,
        );
        painter.rect_filled(
            egui::Rect::from_min_size(rect.min + egui::vec2(GAP, GAP + 6.0), egui::vec2(3.0, 16.0)),
            1.0,
            ACCENT,
        );
        painter.text(
            rect.min + egui::vec2(GAP + 10.0, GAP + HEADER_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(24.0),
            INK,
        );
        for &index in choices {
            let choice = egui::Rect::from_min_size(
                egui::pos2(rect.right() - GAP - 130.0, rect.top() + GAP),
                egui::vec2(130.0, HEADER_HEIGHT),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(choice), |ui| {
                self.choice(ui, index, selected[index]);
            });
        }
        let body_y = GAP + HEADER_HEIGHT + GAP;
        // 上下左右の外周は共通のGAP。余った幅・高さは行内と行間へ均等に配る。
        let row_gap = if rows.len() > 1 {
            (rect.height() - body_y - GAP - KNOB_CELL.y * rows.len() as f32)
                / (rows.len() - 1) as f32
        } else {
            0.0
        };
        for (row, knobs) in rows.iter().enumerate() {
            let width = (rect.width() - GAP * 2.0) / knobs.len() as f32;
            for (k, &index) in knobs.iter().enumerate() {
                let area = egui::Rect::from_min_size(
                    rect.min
                        + egui::vec2(
                            GAP + k as f32 * width,
                            body_y + row as f32 * (KNOB_CELL.y + row_gap),
                        ),
                    egui::vec2(width, KNOB_CELL.y),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    if index == 12 {
                        // Phaseも他のMasterコントロールと同じ行高・中心・ラベル位置を使う。
                        let choice = egui::Rect::from_center_size(
                            egui::pos2(area.center().x, area.top() + 30.0),
                            egui::vec2(120.0, HEADER_HEIGHT),
                        );
                        ui.scope_builder(egui::UiBuilder::new().max_rect(choice), |ui| {
                            self.choice(ui, index, selected[index]);
                        });
                        ui.painter().text(
                            egui::pos2(area.center().x, area.top() + 68.0),
                            egui::Align2::CENTER_CENTER,
                            "Phase",
                            egui::FontId::proportional(18.0),
                            MUTED,
                        );
                    } else {
                        self.knob(ui, index, selected[index], width);
                    }
                });
            }
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
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
            let oscillator_width = GAP * 2.0 + KNOB_CELL.x * 4.0;
            let filter_width = GAP * 2.0 + KNOB_CELL.x * 5.0;
            let single_height = GAP * 3.0 + HEADER_HEIGHT + KNOB_CELL.y;
            let panel_height = single_height * 3.0 + GAP * 2.0;
            let center_x = GAP + oscillator_width + GAP;
            let master_x = center_x + filter_width + GAP;
            let groups = [
                (
                    "Osc1",
                    &[0_usize] as &[usize],
                    &[&[6_usize, 7, 20, 22][..]][..],
                    [GAP, GAP, oscillator_width, single_height],
                ),
                (
                    "Osc2",
                    &[23],
                    &[&[24, 25, 29][..]][..],
                    [
                        GAP,
                        GAP + single_height + GAP,
                        oscillator_width,
                        single_height,
                    ],
                ),
                (
                    "Osc3",
                    &[26],
                    &[&[27, 28, 30][..]][..],
                    [
                        GAP,
                        GAP + (single_height + GAP) * 2.0,
                        oscillator_width,
                        single_height,
                    ],
                ),
                (
                    "Amp",
                    &[],
                    &[&[1, 2, 3, 4, 5][..]][..],
                    [center_x, GAP, filter_width, single_height],
                ),
                (
                    "Filter",
                    &[9],
                    &[&[10, 11, 19][..], &[14, 15, 16, 17, 18][..]][..],
                    [
                        center_x,
                        GAP + single_height + GAP,
                        filter_width,
                        panel_height - single_height - GAP,
                    ],
                ),
                (
                    "Master",
                    &[],
                    &[&[12][..], &[8][..], &[13][..], &[21][..]][..],
                    [master_x, GAP, 192.0, panel_height],
                ),
            ];
            for (title, choices, rows, [x, y, width, height]) in groups {
                self.group(
                    ui,
                    title,
                    choices,
                    rows,
                    egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height)),
                    &selected,
                );
            }
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
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(832.0, 428.0));
        for x in [0.0, 43.0, 416.0, 832.0] {
            for y in [0.0, 114.0, 214.0, 428.0] {
                let knob = egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(50.0, 50.0));
                let bounds = overlay_rect(knob, viewport);
                assert_eq!(bounds.size(), egui::vec2(224.0, 60.0));
                assert!(viewport.contains_rect(bounds));
                if (112.0..=720.0).contains(&x) {
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

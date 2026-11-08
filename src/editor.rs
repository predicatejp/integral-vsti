// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! カテゴリ別のノブ・ドロップダウンとOpenGL描画を接続する。

pub(crate) mod edits;
#[cfg(target_os = "windows")]
pub(crate) mod gl_context;
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
    sync::{
        Arc,
        atomic::{AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// ノブの表示名。ホスト公開名とオーバーレイはparameters::NAMESを使う。
const KNOB_NAMES: [&str; parameters::COUNT] = [
    "Waveform", "A", "H", "D", "S", "R", "Uni", "Det", "Poly", "Filter", "F", "Res", "Phase",
    "Vol", "A", "H", "D", "S", "R", "Amt", "Width", "Pan", "Vol", "Waveform", "Tune", "Vol",
    "Waveform", "Tune", "Vol", "Mod", "Mod", "Clipper", "Gain", "Mix", "Enable", "Low F", "Low G",
    "Mid F", "Mid G", "Mid Q", "High F", "High G",
];

/// 各ノブの1tickに必要な上下移動量（物理px、正の整数）。
const KNOB_PIXELS_PER_TICK: [u32; parameters::COUNT] = [
    4, 3, 3, 3, 4, 3, 32, 4, 8, 4, 2, 2, 4, 2, // Osc1 / Amp / Filter / Master
    3, 3, 3, 4, 3, 2, 4, 4, 2, // Cutoff envelope / Amount / Width / Pan / Osc1 Vol
    4, 3, 2, 4, 3, 2, 4, 4, // Osc2 / Osc3
    4, 3, 4, // Distortion
    4, 4, 4, 4, 4, 4, 4, 4, // eq
];

/// 吸着中の内部移動量の幅（物理px）。0で小さい側、80で大きい側へ抜ける。
const KNOB_DETENT_PIXELS: u32 = 80;

/// 描画目盛りとは独立した吸着対象。最大・最小値は除外する。
fn is_detent(index: usize, tick: u32) -> bool {
    if tick == 0 || tick == parameters::max_tick(index) {
        return false;
    }
    match index {
        10 | 19 | 24 | 27 => tick.is_multiple_of(120), // Cutoff / Amount / Tune: 1200 cent
        13 | 22 | 25 | 28 => tick.is_multiple_of(60),  // Volume: 6 dB
        36 | 38 | 41 => tick.is_multiple_of(60),       // EQ Gain: 6 dB
        21 => tick == 100,                             // Pan: 中央
        _ => false,
    }
}

/// ノブごとの目盛り位置と、長く強調する基準点（ともに整数tick）。
fn knob_marks(index: usize) -> (&'static [u32], u32) {
    match index {
        // 時間: 0 / 10 / 100 / 1000 / 10000 ms。0と隣接する1 msは省く。
        1 | 2 | 3 | 5 | 14 | 15 | 16 | 18 => (&[0, 101, 201, 301, 401], 0),
        4 | 17 => (&[0, 25, 50, 75, 100], 100), // Sustain: 0〜1
        6 => (&[0, 1, 2, 3, 4, 5, 6], 0),       // Unison: 1〜7
        7 | 20 | 33 => (&[0, 25, 50, 75, 100], 0), // Detune / Width / Mix
        8 => (&[0, 7, 15, 23, 31], 0),          // Polyphony: 1/8/16/24/32
        // Cutoff: −2〜+8 oct、0 centを強調。
        10 => (
            &[0, 120, 240, 360, 480, 600, 720, 840, 960, 1080, 1200],
            240,
        ),
        11 => (&[0, 50, 150, 350, 550, 750, 950], 50), // Q: 0.5/1/2/4/6/8/10
        // −∞と−54〜+12 dBの6 dB刻み。全ての吸着位置に目盛りを置く。
        13 | 22 | 25 | 28 => (
            &[0, 60, 120, 180, 240, 300, 360, 420, 480, 540, 600, 660, 720],
            600,
        ),
        19 => (&[0, 120, 240, 360, 480, 600, 720, 840, 960], 480), // Amount: −4〜+4 oct
        21 => (&[0, 50, 100, 150, 200], 100),                      // Pan: 中央を強調
        24 | 27 => (&[0, 120, 240, 360, 480], 240),                // Tune: −2〜+2 oct
        29 | 30 => (&[0, 50, 100, 150, 200], 0),                   // 位相変調: 0〜2回転、半回転刻み
        32 => (&[0, 120, 240, 360, 480], 0),                       // Gain: 0〜48 dB、12 dB刻み
        35 | 37 | 40 => (&[0, 100, 200, 300], 100),
        36 | 38 | 41 => (&[0, 60, 120, 180, 240, 300, 360], 180),
        39 => (&[0, 50, 100, 150, 200], 100),
        _ => (&[], 0),
    }
}

// 操作盤・選択欄・ノブ・オーバーレイで共通の配色。
const PANEL: egui::Color32 = egui::Color32::from_rgb(248, 249, 245);
const INK: egui::Color32 = egui::Color32::from_rgb(23, 43, 38);
const MUTED: egui::Color32 = egui::Color32::from_rgb(53, 71, 62);
const LINE: egui::Color32 = egui::Color32::from_rgb(143, 160, 147);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(20, 96, 76);
const METER_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(224, 230, 224);
const METER_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0, 164, 102);

/// 外周・グループ間・枠内の左右余白と、基本の縦間隔。
const GAP: f32 = 8.0;
/// 見出し行と選択欄の高さ。選択欄は見出し行の中央に置く。
const HEADER_HEIGHT: f32 = 28.0;
const CHOICE_HEIGHT: f32 = 28.0;
/// プラグイン名と発音数を表示する上部バー。
const TITLE_BAR_HEIGHT: f32 = 36.0;
/// ノブとラベルの高さ。横幅は各行で均等に割り当てる。
const KNOB_CELL: egui::Vec2 = egui::vec2(64.0, 80.0);
/// 円盤・ガイド・目盛り・指針の描画倍率。
const KNOB_SCALE: f32 = 0.9;

/// 1論理ポイント当たりの物理px。描画とポインター座標の換算に共用する。
const UI_SCALE: f32 = 1.2;

/// 全パラメーターで共通のオーバーレイ寸法（論理ポイント）。
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
    /// 表示中のサンプルピーク（−60〜＋12 dBFS）。操作値ではない。
    peak_db: f32,
    /// ピーク表示の減衰を実時間で計算する直前の更新時刻。
    meter_updated: Instant,
}

struct DragState {
    #[cfg(target_os = "windows")]
    /// 操作開始時のキー入力先と前面ウィンドウ。所有せず変化の検知だけに使う。
    keyboard_focus: [usize; 2],
    /// 操作対象のパラメーター。
    index: usize,
    /// 操作中の整数tick。ホストへの配送遅延で巻き戻さない。
    tick: u32,
    /// 通常時は未消費の符号付き移動量、吸着中は0〜80の内部位置（物理px）。
    remainder: f32,
    /// 入力開始時のクライアント座標。
    anchor: egui::Pos2,
    /// ホストからの連続値を押下だけで丸めないための変更フラグ。
    changed: bool,
}

/// 吸着区間の両端から隣のtickへ抜ける。余った移動量は次のtickへ持ち越す。
fn move_tick(index: usize, mut tick: u32, mut remainder: f32, dy: f32) -> (u32, f32) {
    let max = parameters::max_tick(index);
    let mut pixels = -dy;
    while pixels != 0.0 {
        let detent = is_detent(index, tick);
        if !detent {
            pixels += remainder;
            remainder = 0.0;
            if (tick == 0 && pixels < 0.0) || (tick == max && pixels > 0.0) {
                return (tick, 0.0);
            }
        }
        let increasing = pixels > 0.0;
        let distance = if detent {
            if increasing {
                KNOB_DETENT_PIXELS as f32 - remainder
            } else {
                remainder
            }
        } else {
            KNOB_PIXELS_PER_TICK[index] as f32
        };
        if pixels.abs() < distance {
            return (tick, remainder + pixels);
        }
        if increasing {
            tick += 1;
            pixels -= distance;
        } else {
            tick -= 1;
            pixels += distance;
        }
        remainder = if is_detent(index, tick) && !increasing {
            KNOB_DETENT_PIXELS as f32
        } else {
            0.0
        };
    }
    (tick, remainder)
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
    /// ボタンイベントへ付与する直近の論理ポイント座標。
    pointer: Cell<egui::Pos2>,
    /// eguiへ渡す単調増加時刻の起点。
    start: Instant,
    /// X11の定期コールバックを有効に保つため、画面と同じ寿命で保持する。
    _timer: baseview::TimerHandle,
    /// Controllerと共有する表示値。
    parameters: Arc<SharedParameters>,
    /// 音声ブロック末尾の発音数。描画側は読み取りだけ行う。
    active_voices: Arc<AtomicU32>,
    /// GUIが受け取るまでの左右最大サンプルピーク。非負f32のビット表現。
    output_peak: Arc<AtomicU32>,
    /// 最後に描画した発音数。変化した場合だけ再描画する。
    displayed_voices: Cell<u32>,
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
        active_voices: Arc<AtomicU32>,
        output_peak: Arc<AtomicU32>,
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
        let timer = window.create_timer(Duration::from_millis(16))?;
        output_peak.store(0, Ordering::Relaxed);
        let editor = Self {
            state: RefCell::new(EditorState {
                drag: None,
                knobs: [egui::Rect::NOTHING; parameters::COUNT],
                peak_db: -60.0,
                meter_updated: Instant::now(),
            }),
            window,
            gl,
            painter: RefCell::new(painter),
            context: egui::Context::default(),
            input: RefCell::new(egui::RawInput::default()),
            pointer: Cell::new(egui::Pos2::ZERO),
            start: Instant::now(),
            _timer: timer,
            parameters,
            active_voices,
            output_peak,
            displayed_voices: Cell::new(u32::MAX),
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
        // 実単位に沿った目盛り。中点を持つパラメーターの円弧はゼロから伸ばす。
        let zero = if matches!(index, 19 | 21 | 24 | 27 | 36 | 38 | 41) {
            0.5
        } else {
            0.0
        };
        let guide_stroke = egui::Stroke::new(2.0 * KNOB_SCALE, LINE);
        let (marks, reference) = knob_marks(index);
        for &mark in marks {
            let d = direction(mark as f32 / parameters::max_tick(index) as f32);
            let major = mark == reference || mark == 0 || mark == parameters::max_tick(index);
            painter.line_segment(
                [
                    center + d * (26.0 * KNOB_SCALE),
                    center + d * (if major { 32.0 } else { 30.0 } * KNOB_SCALE),
                ],
                if major {
                    guide_stroke
                } else {
                    egui::Stroke::new(KNOB_SCALE, LINE)
                },
            );
        }
        let arc = |from: f32, to: f32| {
            (0..=32)
                .map(|k| {
                    center + direction(from + (to - from) * k as f32 / 32.0) * (26.0 * KNOB_SCALE)
                })
                .collect::<Vec<_>>()
        };
        painter.add(egui::Shape::line(arc(0.0, 1.0), guide_stroke));
        painter.add(egui::Shape::line(
            arc(zero, position),
            egui::Stroke::new(3.0 * KNOB_SCALE, ACCENT),
        ));
        painter.circle_filled(
            center + egui::vec2(0.0, 1.5 * KNOB_SCALE),
            22.0 * KNOB_SCALE,
            LINE,
        );
        painter.circle(
            center,
            22.0 * KNOB_SCALE,
            egui::Color32::WHITE,
            egui::Stroke::new(KNOB_SCALE, if drag.is_some() { ACCENT } else { LINE }),
        );
        let d = direction(position);
        painter.line_segment(
            [
                center + d * (10.0 * KNOB_SCALE),
                center + d * (18.0 * KNOB_SCALE),
            ],
            egui::Stroke::new(3.0 * KNOB_SCALE, ACCENT),
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
        let tick = parameters::to_tick(index, selected);
        self.state.borrow_mut().drag = Some(DragState {
            #[cfg(target_os = "windows")]
            keyboard_focus: self.keyboard_focus(),
            index,
            tick,
            remainder: if is_detent(index, tick) {
                KNOB_DETENT_PIXELS as f32 / 2.0
            } else {
                0.0
            },
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
                drag.index,
                drag.tick,
                drag.remainder,
                position.y - drag.anchor.y,
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
        [x, y, width, height]: [f32; 4],
        selected: &[f64; parameters::COUNT],
    ) {
        // 上余白・行間・下余白へ均等配分する。特定の行間だけを引き伸ばさない。
        let choice_height = if matches!(choices, [12] | [31]) {
            CHOICE_HEIGHT
        } else {
            0.0
        };
        let row_header_height = if choices.len() > 1 {
            CHOICE_HEIGHT + GAP
        } else {
            0.0
        };
        let row_height = KNOB_CELL.y + row_header_height;
        let content_rows = rows.len() + usize::from(choice_height > 0.0);
        let row_gap =
            (height - GAP - HEADER_HEIGHT - choice_height - row_height * rows.len() as f32)
                / (content_rows + 1) as f32;
        let body_y = GAP
            + HEADER_HEIGHT
            + row_gap
            + if choice_height > 0.0 {
                choice_height + row_gap
            } else {
                0.0
            };
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
        let off = choices.iter().any(|&index| {
            matches!(index, 9 | 31 | 34) && parameters::to_tick(index, selected[index]) == 0
        });
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
        for (row, knobs) in rows.iter().enumerate() {
            let width = (rect.width() - GAP * 2.0) / knobs.len() as f32;
            let mut left = GAP;
            for &index in *knobs {
                let area = egui::Rect::from_min_size(
                    rect.min
                        + egui::vec2(
                            left,
                            body_y + row as f32 * (row_height + row_gap) + row_header_height,
                        ),
                    egui::vec2(width, KNOB_CELL.y),
                );
                left += width;
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    self.knob(ui, index, selected[index], width);
                });
            }
        }
        let choice_rect = |row: usize| {
            let top = if row_header_height > 0.0 {
                rect.top() + body_y + row as f32 * (row_height + row_gap)
            } else if choice_height > 0.0 {
                rect.top() + GAP + HEADER_HEIGHT + row_gap
            } else {
                rect.top() + GAP + (HEADER_HEIGHT - CHOICE_HEIGHT) / 2.0
            };
            egui::Rect::from_min_size(
                egui::pos2(rect.right() - GAP - 130.0, top),
                egui::vec2(130.0, CHOICE_HEIGHT),
            )
        };
        for (row, &index) in choices.iter().enumerate() {
            let label = match index {
                0 => "Osc1",
                23 => "Osc2",
                26 => "Osc3",
                12 => "Phase",
                31 => "Type",
                _ => continue,
            };
            painter.text(
                egui::pos2(rect.left() + GAP, choice_rect(row).center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(18.0),
                MUTED,
            );
        }
        if off {
            // 一様な背景色を重ね、描画済みグループ全体を50%で合成する。
            // 個々の図形を透過すると、本来隠れるノブの影まで透けてしまう。
            painter.rect_filled(
                rect.expand(1.0),
                0.0,
                ui.visuals().panel_fill.gamma_multiply(0.5),
            );
        }
        for (row, &index) in choices.iter().enumerate() {
            ui.scope_builder(egui::UiBuilder::new().max_rect(choice_rect(row)), |ui| {
                self.choice(ui, index, selected[index]);
            });
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
        // 借用をフレーム全体で保持し、OSやドライバーからの再入は次の描画へ回す。
        // 失敗時の早期returnでもRefMutが解放され、次の描画を受け付けられる。
        let Ok(mut painter) = self.painter.try_borrow_mut() else {
            // 再入側のフレーム終了に再描画要求を消費させず、復帰後に配送する。
            self.window.request_redraw_after(Duration::ZERO);
            return Ok(());
        };
        let size = self.window.size().physical;
        let mut input = self.input.borrow_mut().take();
        input.time = Some(self.start.elapsed().as_secs_f64());
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .native_pixels_per_point = Some(UI_SCALE);
        input.screen_rect = Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(size.width as f32, size.height as f32) / UI_SCALE,
        ));
        input.max_texture_side = Some(painter.max_texture_side());
        let selected = self.parameters.snapshot();
        self.displayed.set(selected);
        let voice_count = self.active_voices.load(Ordering::Relaxed);
        self.displayed_voices.set(voice_count);
        let mut output = self.context.run_ui(input, |ui| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
            let title = egui::Rect::from_min_size(
                egui::vec2(GAP, GAP).to_pos2(),
                egui::vec2(size.width as f32 / UI_SCALE - GAP * 2.0, TITLE_BAR_HEIGHT),
            );
            let painter = ui.painter();
            painter.rect_filled(title, 8.0, PANEL);
            painter.text(
                title.left_center() + egui::vec2(GAP, 0.0),
                egui::Align2::LEFT_CENTER,
                "Sample VSTi",
                egui::FontId::proportional(24.0),
                INK,
            );
            let meter_x = title.right() - GAP - 254.0;
            let peak_db = self.state.borrow().peak_db;
            let peak_rect = egui::Rect::from_min_size(
                egui::pos2(meter_x, title.center().y - 10.0),
                egui::vec2(254.0, 8.0),
            );
            painter.rect_filled(peak_rect, 1.0, METER_BACKGROUND);
            let zero_x = meter_x + peak_rect.width() * 60.0 / 72.0;
            for (start_db, end_db, color) in [
                (-60.0, 0.0, METER_ACTIVE),
                (0.0, 12.0, egui::Color32::from_rgb(232, 64, 48)),
            ] {
                let width = peak_rect.width() * (peak_db.min(end_db) - start_db).max(0.0) / 72.0;
                if width > 0.0 {
                    let start_x = meter_x + peak_rect.width() * (start_db + 60.0) / 72.0;
                    painter.rect_filled(
                        egui::Rect::from_min_size(
                            egui::pos2(start_x, peak_rect.top()),
                            egui::vec2(width, 8.0),
                        ),
                        0.0,
                        color,
                    );
                }
            }
            painter.vline(zero_x, peak_rect.y_range(), egui::Stroke::new(1.0, INK));
            for voice in 0..32 {
                let segment = egui::Rect::from_min_size(
                    egui::pos2(meter_x + voice as f32 * 8.0, title.center().y + 2.0),
                    egui::vec2(6.0, 8.0),
                );
                painter.rect_filled(
                    segment,
                    1.0,
                    if voice < voice_count {
                        METER_ACTIVE
                    } else {
                        METER_BACKGROUND
                    },
                );
            }
            let oscillator_width = GAP * 2.0 + KNOB_CELL.x * 4.0;
            let filter_width = GAP * 2.0 + KNOB_CELL.x * 5.0;
            let panel_height = 500.0;
            let center_x = GAP + oscillator_width + GAP;
            let effects_x = center_x + filter_width + GAP;
            let side_width = GAP * 2.0 + KNOB_CELL.x * 3.0;
            let master_x = effects_x + side_width + GAP;
            // AmpとFilterの上余白・行間・下余白を同じ長さに揃える。
            let center_gap =
                (panel_height - GAP - 2.0 * (GAP + HEADER_HEIGHT) - KNOB_CELL.y * 3.0) / 5.0;
            let amp_height = GAP + HEADER_HEIGHT + KNOB_CELL.y + center_gap * 2.0;
            let filter_height = panel_height - amp_height - GAP;
            let effect_height = GAP * 4.0 + HEADER_HEIGHT + CHOICE_HEIGHT + KNOB_CELL.y;
            let eq_height = GAP * 5.0 + HEADER_HEIGHT + KNOB_CELL.y * 3.0;
            let groups = [
                (
                    "Oscillator",
                    &[0_usize, 23, 26] as &[usize],
                    &[
                        &[6_usize, 7, 20, 22][..],
                        &[24, 25, 29][..],
                        &[27, 28, 30][..],
                    ][..],
                    [GAP, GAP, oscillator_width, panel_height],
                ),
                (
                    "Amp",
                    &[],
                    &[&[1, 2, 3, 4, 5][..]][..],
                    [center_x, GAP, filter_width, amp_height],
                ),
                (
                    "Filter",
                    &[9],
                    &[&[10, 11, 19][..], &[14, 15, 16, 17, 18][..]][..],
                    [
                        center_x,
                        GAP + amp_height + GAP,
                        filter_width,
                        filter_height,
                    ],
                ),
                (
                    "Distortion",
                    &[31],
                    &[&[32, 33][..]][..],
                    [effects_x, GAP, side_width, effect_height],
                ),
                (
                    "EQ",
                    &[34],
                    &[&[35, 36][..], &[37, 38, 39][..], &[40, 41][..]][..],
                    [master_x, GAP, side_width, eq_height],
                ),
                (
                    "Master",
                    &[12],
                    &[&[8, 13, 21][..]][..],
                    [
                        master_x,
                        GAP + eq_height + GAP,
                        side_width,
                        panel_height - eq_height - GAP,
                    ],
                ),
            ];
            for (title, choices, rows, [x, y, width, height]) in groups {
                self.group(
                    ui,
                    title,
                    choices,
                    rows,
                    [x, y + TITLE_BAR_HEIGHT + GAP, width, height],
                    &selected,
                );
            }
        });
        let shapes = self
            .context
            .tessellate(output.shapes, output.pixels_per_point);
        #[cfg(target_os = "windows")]
        let _restore = gl_context::RestoreContext::capture();
        // SAFETY: GL処理をこの描画呼び出し内に限定する。Windowsでは元の設定を復元する。
        unsafe {
            self.gl.make_current()?;
        }
        painter.clear([size.width, size.height], [0.0, 0.0, 0.0, 1.0]);
        painter.paint_and_update_textures(
            [size.width, size.height],
            output.pixels_per_point,
            &shapes,
            &mut output.textures_delta,
        );
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
                let pos = egui::pos2(position.x as f32 / UI_SCALE, position.y as f32 / UI_SCALE);
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
        // 描画中のタイマー再入ではホストを呼ばず、編集キューを次の通知まで保持する。
        if self.painter.try_borrow().is_err() {
            return;
        }
        // キーボードフォーカスを取得しないWindowsでは、終了条件もタイマーで確認する。
        #[cfg(target_os = "windows")]
        if self.state.borrow().drag.is_some() && self.current_drag_position().is_none() {
            self.end_drag();
        }
        let peak_changed = {
            let incoming = f32::from_bits(self.output_peak.swap(0, Ordering::Relaxed));
            let mut state = self.state.borrow_mut();
            let now = Instant::now();
            let elapsed = now.duration_since(state.meter_updated).as_secs_f32();
            state.meter_updated = now;
            let peak_db = (20.0 * incoming.max(0.001).log10())
                .min(12.0)
                .max(state.peak_db - 48.0 * elapsed)
                .max(-60.0);
            let changed = peak_db != state.peak_db;
            state.peak_db = peak_db;
            changed
        };
        if peak_changed
            || self.displayed.get() != self.parameters.snapshot()
            || self.displayed_voices.get() != self.active_voices.load(Ordering::Relaxed)
        {
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
        #[cfg(target_os = "windows")]
        let _restore = gl_context::RestoreContext::capture();
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
        let viewport = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(784.0, 560.0));
        for x in [0.0, 43.0, 392.0, 784.0] {
            for y in [0.0, 114.0, 280.0, 560.0] {
                let knob = egui::Rect::from_center_size(egui::pos2(x, y), egui::vec2(50.0, 50.0));
                let bounds = overlay_rect(knob, viewport);
                assert_eq!(bounds.size(), egui::vec2(224.0, 60.0));
                assert!(viewport.contains_rect(bounds));
                if (112.0..=672.0).contains(&x) {
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
        for (index, &pixels) in KNOB_PIXELS_PER_TICK.iter().enumerate().skip(1) {
            if !parameters::choices(index).is_empty() {
                continue;
            }
            let max = parameters::max_tick(index);
            for tick in 0..=max {
                let initial = if is_detent(index, tick) { 40.0 } else { 0.0 };
                let distance = if is_detent(index, tick) {
                    40.0
                } else {
                    pixels as f32
                };
                for (dy, next) in [(-distance, tick + 1), (distance, tick.saturating_sub(1))] {
                    if (tick == 0 && dy > 0.0) || (tick == max && dy < 0.0) {
                        continue;
                    }
                    let partial = dy - dy.signum();
                    let held = move_tick(index, tick, initial, partial);
                    assert_eq!(held, (tick, initial - partial));
                    let entry = if is_detent(index, next) && dy > 0.0 {
                        80.0
                    } else {
                        0.0
                    };
                    assert_eq!(move_tick(index, held.0, held.1, dy.signum()), (next, entry));
                }
            }
            assert_eq!(move_tick(index, max, 0.0, -100.0), (max, 0.0));
            assert_eq!(move_tick(index, 0, 0.0, 100.0), (0, 0.0));
        }
    }

    #[test]
    fn detents_are_limited_to_pitch_volume_eq_gain_and_centers() {
        for index in 0..parameters::COUNT {
            for tick in 0..=parameters::max_tick(index) {
                let targets: &[u32] = match index {
                    10 => &[120, 240, 360, 480, 600, 720, 840, 960, 1080],
                    24 | 27 => &[120, 240, 360],
                    13 | 22 | 25 | 28 => &[60, 120, 180, 240, 300, 360, 420, 480, 540, 600, 660],
                    19 => &[120, 240, 360, 480, 600, 720, 840],
                    21 => &[100],
                    36 | 38 | 41 => &[60, 120, 180, 240, 300],
                    _ => &[],
                };
                if is_detent(index, tick) {
                    assert!(
                        knob_marks(index).0.contains(&tick),
                        "missing mark: {index}: {tick}"
                    );
                }
                assert_eq!(
                    is_detent(index, tick),
                    targets.contains(&tick),
                    "{index}: {tick}"
                );
            }
        }
    }

    #[test]
    fn detent_position_tracks_entry_direction_and_reversal() {
        assert_eq!(move_tick(21, 99, 0.0, -4.0), (100, 0.0));
        assert_eq!(move_tick(21, 100, 0.0, 0.0), (100, 0.0));
        assert_eq!(move_tick(21, 100, 0.0, -79.0), (100, 79.0));
        assert_eq!(move_tick(21, 100, 79.0, -1.0), (101, 0.0));
        assert_eq!(move_tick(21, 101, 0.0, 4.0), (100, 80.0));
        assert_eq!(move_tick(21, 100, 80.0, 0.0), (100, 80.0));
        assert_eq!(move_tick(21, 100, 80.0, 79.0), (100, 1.0));
        assert_eq!(move_tick(21, 100, 1.0, 1.0), (99, 0.0));
        // 進入した側へすぐ戻す場合、80pxの移動を要求しない。
        assert_eq!(move_tick(21, 100, 0.0, 1.0), (99, -1.0));
        assert_eq!(move_tick(21, 100, 80.0, -1.0), (101, 1.0));
        // 内部で20px進んでから20px戻すと、小さい側へ抜ける。
        assert_eq!(move_tick(21, 100, 0.0, -20.0), (100, 20.0));
        assert_eq!(move_tick(21, 100, 20.0, 19.0), (100, 1.0));
        assert_eq!(move_tick(21, 100, 1.0, 1.0), (99, 0.0));
        // 吸着点から開始する場合は中間の40pxから両方向へ抜ける。
        for dy in [-39.0_f32, 39.0] {
            let held = move_tick(21, 100, 40.0, dy);
            assert_eq!(held, (100, 40.0 - dy));
            assert_eq!(
                move_tick(21, held.0, held.1, dy.signum()),
                (if dy < 0.0 { 101 } else { 99 }, 0.0)
            );
        }
        assert_eq!(move_tick(21, 99, 0.0, -88.0), (102, 0.0));
        for index in [10, 13, 19, 21, 24, 36, 38, 41] {
            for dy in [-700.0_f32, 700.0] {
                let tick = parameters::max_tick(index) / 2;
                let initial = if is_detent(index, tick) { 40.0 } else { 0.0 };
                let whole = move_tick(index, tick, initial, dy);
                let mut split = (tick, initial);
                for _ in 0..700 {
                    split = move_tick(index, split.0, split.1, dy.signum());
                }
                assert_eq!(whole, split);
            }
        }
    }
}

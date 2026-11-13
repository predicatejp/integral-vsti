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
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

/// ノブの表示名。ホスト公開名とオーバーレイはparameters::NAMESを使う。
const KNOB_NAMES: [&str; parameters::COUNT] = [
    "Waveform", "A", "H", "D", "S", "R", "Uni", "Det", "Poly", "Filter", "F", "Res", "Phase",
    "Vol", "A", "H", "D", "S", "R", "Amt", "Width", "Pan", "Vol", "Waveform", "Tune", "Vol",
    "Waveform", "Tune", "Vol", "Mod", "Mod", "Clipper", "Gain", "Mix", "Enable", "Low F", "Low G",
    "Mid F", "Mid G", "Mid Q", "High F", "High G", "Effect 1", "Effect 2", "Effect 3", "Clipper",
    "Gain", "Mix", "Clipper", "Gain", "Mix", "Time", "Fdbk", "Mix", "Time", "Fdbk", "Mix", "Time",
    "Fdbk", "Mix", "Stereo", "Stereo", "Stereo", "Dec", "Damp", "Size", "Mix", "Dec", "Damp",
    "Size", "Mix", "Dec", "Damp", "Size", "Mix", "Time", "Depth", "Rate", "Det", "Fdbk", "Mix",
    "Time", "Depth", "Rate", "Det", "Fdbk", "Mix", "Time", "Depth", "Rate", "Det", "Fdbk", "Mix",
    "Center", "Depth", "Rate", "Fdbk", "Mix", "Center", "Depth", "Rate", "Fdbk", "Mix", "Center",
    "Depth", "Rate", "Fdbk", "Mix", "Limiter", "Thrs", "Ratio", "A", "R", "Knee", "Gain", "Mix",
    "Thrs", "Ratio", "A", "R", "Knee", "Gain", "Mix", "Thrs", "Ratio", "A", "R", "Knee", "Gain",
    "Mix", "Sens", "Sens", "Porta", "Pitch",
];

/// 各ノブの1tickに必要な上下移動量（物理px、正の整数）。
const KNOB_PIXELS_PER_TICK: [u32; parameters::COUNT] = [
    4, 3, 3, 3, 4, 3, 32, 4, 8, 4, 2, 2, 4, 2, // Osc1 / Amp / Filter / Master
    3, 3, 3, 4, 3, 2, 4, 4, 2, // Cutoff envelope / Amount / Width / Pan / Osc1 Vol
    4, 3, 2, 4, 3, 2, 4, 4, // Osc2 / Osc3
    4, 3, 4, // Distortion
    4, 4, 4, 4, 4, 4, 4, 4, // eq
    4, 4, 4, 4, 3, 4, 4, 3, 4, // Effects
    32, 4, 4, 32, 4, 4, 32, 4, 4, // Delay
    4, 4, 4, // Stereo
    4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, // Reverb
    4, 2, 3, 4, 4, 4, 4, 2, 3, 4, 4, 4, 4, 2, 3, 4, 4, 4, // Chorus
    3, 4, 3, 4, 4, 3, 4, 3, 4, 4, 3, 4, 3, 4, 4, // Phaser
    4, // Limiter
    2, 4, 3, 3, 4, 4, 4, // Compressor
    2, 4, 3, 3, 4, 4, 4, // Compressor
    2, 4, 3, 3, 4, 4, 4, // Compressor
    4, 4, // Velocity sensitivity
    3, 4, // Portamento / Pitch
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
        36 | 38 | 41 | 114 | 121 | 128 => tick.is_multiple_of(60), // EQ Gain / Makeup: 6 dB
        21 => tick == 100,                             // Pan: 中央
        _ => false,
    }
}

/// ノブごとの目盛り位置と、長く強調する基準点（ともに整数tick）。
fn knob_marks(index: usize) -> (&'static [u32], u32) {
    match parameters::control_kind(index) {
        // 時間: 0 / 10 / 100 / 1000 / 10000 ms。0と隣接する1 msは省く。
        1 | 2 | 3 | 5 | 14 | 15 | 16 | 18 => (&[0, 101, 201, 301, 401], 0),
        4 | 17 => (&[0, 25, 50, 75, 100], 100), // Sustain: 0〜1
        6 => (&[0, 1, 2, 3, 4, 5, 6], 0),       // Unison: 1〜7
        7 | 20 | 33 | 52 | 130 | 131 => (&[0, 25, 50, 75, 100], 0), // Detune / Width / Mix
        parameters::PITCH => (&[0, 20, 40, 60, 80, 100, 120], 0),
        8 => (&[0, 7, 15, 23, 31], 0), // Polyphony: 1/8/16/24/32
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
        109 => (&[0, 120, 240, 360, 480, 600], 600), // Threshold: −60〜0 dB
        110 => (&[0, 10, 30, 70, 190], 0),           // Ratio: 1/2/4/8/20
        113 | 114 => (&[0, 60, 120, 180, 240], 0),   // Knee / Makeup: 6 dB
        93 => (&[0, 100, 200, 300, 400], 0),
        94 => (&[0, 60, 120, 180, 240], 0),
        75 => (&[0, 9, 99, 199, 399], 199), // Time: 1/10/20/40ms
        76 => (&[0, 100, 200, 500, 1000], 0),
        77 => (&[0, 45, 95, 195, 495], 95),
        78 => (&[0, 50, 100, 150, 200], 100),
        79 => (&[0, 45, 95, 145, 190], 95),
        63 => (&[0, 50, 100, 150, 200], 100), // Decay: 1s
        64 => (&[0, 75, 150, 225, 300], 0),
        65 => (&[0, 50, 100, 150], 50), // Size: 100%
        51 => (&[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13], 5), // Delay Time: 1/8を基準にする。
        _ => (&[], 0),
    }
}

// 操作盤・選択欄・ノブ・オーバーレイで共通の配色。
const PANEL: egui::Color32 = egui::Color32::from_rgb(248, 249, 245);
const INK: egui::Color32 = egui::Color32::from_rgb(23, 43, 38);
const MUTED: egui::Color32 = egui::Color32::from_rgb(53, 71, 62);
const SECTION_TITLE: egui::Color32 = egui::Color32::from_rgb(108, 123, 113);
const LINE: egui::Color32 = egui::Color32::from_rgb(143, 160, 147);
const ACCENT: egui::Color32 = egui::Color32::from_rgb(20, 96, 76);
const METER_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(224, 230, 224);
const METER_ACTIVE: egui::Color32 = egui::Color32::from_rgb(0, 164, 102);

/// 外周・グループ間・枠内の左右余白と、基本の縦間隔。
const GAP: f32 = 8.0;
/// 見出し行と選択欄の高さ。選択欄は見出し行の中央に置く。
const HEADER_HEIGHT: f32 = 28.0;
const CHOICE_HEIGHT: f32 = 28.0;
/// 製品名・全体コントロール・メーターを表示する上部バー。
const TITLE_BAR_HEIGHT: f32 = KNOB_CELL.y + GAP * 2.0;
/// ノブとラベルの高さ。横幅は各行で均等に割り当てる。
const KNOB_CELL: egui::Vec2 = egui::vec2(56.0, 64.0);
const KNOB_CENTER_Y: f32 = 28.0;
const FONT_SIZE: f32 = 16.0;
const HEADING_FONT_SIZE: f32 = 22.0;
/// 円盤・ガイド・目盛り・指針の描画倍率。
const KNOB_SCALE: f32 = 0.82;

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

/// 各パネルの共通枠。見出しはOsc1・Eff 1と同じ小さいスタイル。
fn panel(painter: &egui::Painter, rect: egui::Rect, title: &str) {
    painter.rect_filled(rect, 8.0, PANEL);
    painter.rect_stroke(
        rect,
        8.0,
        egui::Stroke::new(1.0, LINE),
        egui::StrokeKind::Inside,
    );
    if !title.is_empty() {
        painter.text(
            rect.min + egui::vec2(GAP, GAP + HEADER_HEIGHT / 2.0),
            egui::Align2::LEFT_CENTER,
            title,
            egui::FontId::proportional(FONT_SIZE),
            SECTION_TITLE,
        );
    }
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
    /// 描画側から音声側へ渡すDSPリセット要求。
    reset_requested: Arc<AtomicBool>,
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
        reset_requested: Arc<AtomicBool>,
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
            reset_requested,
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
                    HEADING_FONT_SIZE
                } else {
                    FONT_SIZE
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
            egui::pos2(area.center().x, area.top() + KNOB_CENTER_Y),
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
        let zero = if matches!(
            parameters::control_kind(index),
            19 | 21 | 24 | 27 | 36 | 38 | 41 | 78 | 79
        ) {
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
        let (label_position, label_align) = if matches!(index, 13 | 21) {
            (
                egui::pos2(area.left() - GAP, center.y),
                egui::Align2::RIGHT_CENTER,
            )
        } else {
            (
                egui::pos2(center.x, area.top() + 56.0),
                egui::Align2::CENTER_CENTER,
            )
        };
        painter.text(
            label_position,
            label_align,
            KNOB_NAMES[index],
            egui::FontId::proportional(FONT_SIZE),
            MUTED,
        );
        if drag.is_some() {
            let value_label = if let Some(label) = parameters::choices(index).get(tick as usize) {
                (*label).to_owned()
            } else if parameters::is_volume(index) && value == f64::NEG_INFINITY {
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
                egui::FontId::proportional(FONT_SIZE),
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
        if matches!(index, 12 | 31 | 45 | 48 | 60..=62) {
            ui.painter().text(
                ui.max_rect().center_top() - egui::vec2(0.0, GAP),
                egui::Align2::CENTER_BOTTOM,
                KNOB_NAMES[index],
                egui::FontId::proportional(FONT_SIZE),
                MUTED,
            );
        }
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

    fn note_group(&self, ui: &mut egui::Ui, rect: egui::Rect, selected: &[f64; parameters::COUNT]) {
        panel(ui.painter(), rect, "Note");
        let choice_width = 112.0;
        let width = (rect.width() - GAP * 2.0 - choice_width) / 3.0;
        let top = rect.top() + GAP * 2.0 + HEADER_HEIGHT;
        for (column, index) in [12, 8, parameters::PORTAMENTO, parameters::PITCH]
            .into_iter()
            .enumerate()
        {
            let left = rect.left() + GAP;
            let area = if index == 12 {
                egui::Rect::from_min_size(
                    egui::pos2(left, top + KNOB_CENTER_Y - CHOICE_HEIGHT / 2.0 + GAP),
                    egui::vec2(choice_width - GAP, CHOICE_HEIGHT),
                )
            } else {
                egui::Rect::from_min_size(
                    egui::pos2(left + choice_width + (column - 1) as f32 * width, top),
                    egui::vec2(width, KNOB_CELL.y),
                )
            };
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                if index == 12 {
                    self.choice(ui, index, selected[index]);
                } else {
                    self.knob(ui, index, selected[index], width);
                }
            });
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
        let effect = parameters::EFFECT_SLOTS
            .iter()
            .position(|indices| choices.first() == Some(&indices[0]));
        // 全エフェクト共通のMixは右端の同じ位置へ分離する。
        let (mix, knobs) = if effect.is_some() {
            rows[0]
                .split_last()
                .map_or((None, &[][..]), |(mix, knobs)| (Some(*mix), knobs))
        } else {
            (None, rows[0])
        };
        let columns = match knobs.first().map(|&index| parameters::control_kind(index)) {
            Some(51) => 1, // Delay: Time / Feedbackを縦に並べる。
            Some(93) => 2, // Phaser: Center / Depth、Rate / Feedbackの2×2。
            _ => 3,
        };
        let split = knobs.len().min(columns);
        let two_rows = [&knobs[..split], &knobs[split..]];
        let rows = if effect.is_some() {
            &two_rows[..if knobs.len() > split { 2 } else { 1 }]
        } else {
            rows
        };
        // 上余白・行間・下余白へ均等配分する。特定の行間だけを引き伸ばさない。
        let internal_choice_width = if effect.is_some() && choices.len() > 1 {
            90.0 + GAP
        } else {
            0.0
        };
        let row_header_height = if choices.len() > 1 && effect.is_none() {
            CHOICE_HEIGHT + GAP
        } else {
            0.0
        };
        let row_height = KNOB_CELL.y + row_header_height;
        let header_height = if title.is_empty() { 0.0 } else { HEADER_HEIGHT };
        let row_gap = (height - GAP - header_height - row_height * rows.len() as f32)
            / (rows.len() + 1) as f32;
        let body_y = if row_header_height > 0.0 {
            GAP
        } else {
            GAP + header_height + row_gap
        };
        let row_step = if row_header_height > 0.0 {
            height / rows.len() as f32
        } else {
            row_height + row_gap
        };
        let knob_offset = if row_header_height > 0.0 {
            (row_step - GAP * 2.0 - row_height) / 2.0
        } else {
            0.0
        };
        let rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(width, height));
        let centered_row_y = (height + GAP + HEADER_HEIGHT - KNOB_CELL.y) / 2.0;
        let off = choices.iter().any(|&index| {
            matches!(index, 9 | 34 | 42..=44) && parameters::to_tick(index, selected[index]) == 0
        });
        let painter = ui.painter().clone();
        if let Some(slot) = effect {
            if slot > 0 {
                painter.hline(
                    rect.left() + GAP..=rect.right() - GAP,
                    rect.top(),
                    egui::Stroke::new(1.0, LINE),
                );
            }
            painter.text(
                rect.min + egui::vec2(GAP, GAP + HEADER_HEIGHT / 2.0),
                egui::Align2::LEFT_CENTER,
                title,
                egui::FontId::proportional(FONT_SIZE),
                SECTION_TITLE,
            );
        } else {
            panel(&painter, rect, title);
        }
        for (row, knobs) in rows.iter().enumerate() {
            if knobs.is_empty() {
                continue;
            }
            let row_top = body_y + row as f32 * row_step;
            if row > 0 && row_header_height > 0.0 {
                painter.hline(
                    rect.left() + GAP..=rect.right() - GAP,
                    rect.top() + row_top - GAP,
                    egui::Stroke::new(1.0, LINE),
                );
            }
            let width = (rect.width()
                - GAP * 2.0
                - internal_choice_width
                - if effect.is_some() {
                    KNOB_CELL.x + GAP * 2.0
                } else {
                    0.0
                })
                / knobs.len() as f32;
            let mut left = GAP + internal_choice_width;
            for &index in *knobs {
                let area = egui::Rect::from_min_size(
                    rect.min + egui::vec2(left, row_top + row_header_height + knob_offset),
                    egui::vec2(width, KNOB_CELL.y),
                );
                // EQのMid・Highの手前で帯域を区切る。
                if matches!(index, 37 | 40) {
                    painter.vline(area.left(), area.y_range(), egui::Stroke::new(1.0, LINE));
                }
                left += width;
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    self.knob(ui, index, selected[index], width);
                });
            }
        }
        if let Some(index) = mix {
            let area = egui::Rect::from_min_size(
                egui::pos2(
                    rect.right() - GAP - KNOB_CELL.x,
                    rect.top() + centered_row_y,
                ),
                KNOB_CELL,
            );
            painter.vline(
                area.left() - GAP,
                rect.top() + HEADER_HEIGHT + GAP * 2.0..=rect.bottom() - GAP,
                egui::Stroke::new(1.0, LINE),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                self.knob(ui, index, selected[index], KNOB_CELL.x);
            });
        }
        let choice_rect = |row: usize| {
            if effect.is_some() && row > 0 {
                return egui::Rect::from_min_size(
                    rect.min
                        + egui::vec2(GAP, centered_row_y + KNOB_CENTER_Y - CHOICE_HEIGHT / 2.0),
                    egui::vec2(internal_choice_width - GAP, CHOICE_HEIGHT),
                );
            }
            let top = if effect.is_some() && row == 0 {
                rect.top() + GAP
            } else if row_header_height > 0.0 {
                rect.top() + body_y + row as f32 * row_step
            } else {
                rect.top() + GAP + (HEADER_HEIGHT - CHOICE_HEIGHT) / 2.0
            };
            let width = (rect.width() - GAP * 2.0).min(if effect.is_some() && row == 0 {
                150.0
            } else {
                130.0
            });
            egui::Rect::from_min_size(
                egui::pos2(
                    rect.right()
                        - GAP
                        - width
                        - if effect.is_some() && row == 0 {
                            60.0
                        } else {
                            0.0
                        },
                    top,
                ),
                egui::vec2(width, CHOICE_HEIGHT),
            )
        };
        for (row, &index) in choices.iter().enumerate() {
            let label = match index {
                0 => "Osc1",
                23 => "Osc2",
                26 => "Osc3",
                _ => continue,
            };
            painter.text(
                egui::pos2(rect.left() + GAP, choice_rect(row).center().y),
                egui::Align2::LEFT_CENTER,
                label,
                egui::FontId::proportional(FONT_SIZE),
                SECTION_TITLE,
            );
        }
        if off {
            // 背景色との50%合成により、影や文字を含む全体を同じ率で薄くする。
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
        if let Some(slot) = effect {
            // 端では対応する方向だけ無効化する。選択と内部設定を一緒に移動する。
            for direction in 0..2 {
                let area = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.right() - GAP - 52.0 + direction as f32 * 28.0,
                        choice_rect(0).top(),
                    ),
                    egui::vec2(24.0, CHOICE_HEIGHT),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    let enabled = if direction == 0 { slot > 0 } else { slot < 2 };
                    let response =
                        ui.add_enabled(enabled, egui::Button::new("").min_size(area.size()));
                    let center = response.rect.center();
                    let sign = if direction == 0 { -1.0 } else { 1.0 };
                    ui.painter().add(egui::Shape::convex_polygon(
                        vec![
                            center + egui::vec2(0.0, sign * 5.0),
                            center + egui::vec2(-5.0, -sign * 3.0),
                            center + egui::vec2(5.0, -sign * 3.0),
                        ],
                        if enabled { ACCENT } else { LINE },
                        egui::Stroke::NONE,
                    ));
                    if response.clicked() {
                        self.edits.push(Edit::SwapEffects(if direction == 0 {
                            slot - 1
                        } else {
                            slot
                        }));
                        response.surrender_focus();
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
        // 種類切替で非表示になったノブの編集を終了し、当たり判定を描画中の要素だけにする。
        let hidden_drag = self.state.borrow().drag.as_ref().is_some_and(|drag| {
            parameters::EFFECT_SLOTS.iter().any(|indices| {
                let visible = match parameters::to_plain(indices[0], selected[indices[0]]) as usize
                {
                    1 => &indices[12..18],
                    2 => &indices[23..],
                    3 => &indices[4..7],
                    4 => &indices[2..4],
                    5 => &indices[18..23],
                    6 => &indices[8..12],
                    _ => &[],
                };
                indices.contains(&drag.index) && !visible.contains(&drag.index)
            })
        });
        if hidden_drag {
            self.end_drag();
        }
        self.state.borrow_mut().knobs.fill(egui::Rect::NOTHING);
        self.displayed.set(selected);
        let voice_count = self.active_voices.load(Ordering::Relaxed);
        self.displayed_voices.set(voice_count);
        let mut output = self.context.run_ui(input, |ui| {
            ui.painter()
                .rect_filled(ui.max_rect(), 0.0, ui.visuals().panel_fill);
            let oscillator_width = GAP * 2.0 + KNOB_CELL.x * 4.0;
            let filter_width = GAP * 2.0 + KNOB_CELL.x * 6.0;
            let center_x = GAP + oscillator_width + GAP;
            let effects_x = center_x + filter_width + GAP;
            let title = egui::Rect::from_min_size(
                egui::vec2(GAP, GAP).to_pos2(),
                egui::vec2(size.width as f32 / UI_SCALE - GAP * 2.0, TITLE_BAR_HEIGHT),
            );
            let painter = ui.painter().clone();
            painter.rect_filled(title, 8.0, PANEL);
            painter.text(
                title.left_center() + egui::vec2(GAP, 0.0),
                egui::Align2::LEFT_CENTER,
                "Integral",
                egui::FontId::proportional(HEADING_FONT_SIZE),
                INK,
            );
            let meter_x = title.right() - GAP - 254.0;
            let button_size = egui::vec2(80.0, 48.0);
            let reset_rect = egui::Rect::from_min_size(
                egui::pos2(
                    meter_x - GAP - button_size.x,
                    title.center().y - button_size.y / 2.0,
                ),
                button_size,
            );
            let limiter_rect = egui::Rect::from_min_size(
                egui::pos2(reset_rect.left() - GAP - button_size.x, reset_rect.top()),
                button_size,
            );
            // ラベル32pxとノブの間はGAP、各組の左右余白は2×GAP。
            let control_width = 32.0 + GAP + KNOB_CELL.x + GAP * 4.0;
            for (column, index) in [13, 21].into_iter().enumerate() {
                let right = limiter_rect.left() - GAP * 2.0 - control_width * (1 - column) as f32;
                let area = egui::Rect::from_min_size(
                    egui::pos2(
                        right - GAP * 2.0 - KNOB_CELL.x,
                        title.center().y - KNOB_CENTER_Y,
                    ),
                    KNOB_CELL,
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(area), |ui| {
                    self.knob(ui, index, selected[index], KNOB_CELL.x);
                });
            }
            let limiter_tick = parameters::to_tick(108, selected[108]);
            let limiter = ui.put(
                limiter_rect,
                egui::Button::new("Limiter").selected(limiter_tick != 0),
            );
            if limiter.clicked() {
                self.edits.push(Edit::Set(
                    108,
                    parameters::to_normalized(108, f64::from(1 - limiter_tick)),
                ));
                limiter.surrender_focus();
            }
            let reset = ui.put(reset_rect, egui::Button::new("Reset"));
            if reset.clicked() {
                self.reset_requested.store(true, Ordering::Relaxed);
                self.state.borrow_mut().peak_db = -60.0;
                reset.surrender_focus();
            }
            let peak_db = self.state.borrow().peak_db;
            let meter_height = (button_size.y - GAP) / 2.0;
            let peak_rect = egui::Rect::from_min_size(
                egui::pos2(meter_x, reset_rect.top()),
                egui::vec2(254.0, meter_height),
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
                            egui::vec2(width, meter_height),
                        ),
                        0.0,
                        color,
                    );
                }
            }
            painter.vline(zero_x, peak_rect.y_range(), egui::Stroke::new(1.0, INK));
            for voice in 0..32 {
                let segment = egui::Rect::from_min_size(
                    egui::pos2(meter_x + voice as f32 * 8.0, peak_rect.bottom() + GAP),
                    egui::vec2(6.0, meter_height),
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
            let left_top = title.bottom() + GAP;
            let full_height = size.height as f32 / UI_SCALE - left_top - GAP;
            let eq_height = GAP * 3.0 + HEADER_HEIGHT + KNOB_CELL.y;
            let note_height = GAP * 3.0 + HEADER_HEIGHT + KNOB_CELL.y;
            let amp_top = left_top + note_height + GAP;
            let panel_height = full_height - GAP - eq_height;
            self.note_group(
                ui,
                egui::Rect::from_min_size(
                    egui::pos2(center_x, left_top),
                    egui::vec2(filter_width, note_height),
                ),
                &selected,
            );
            // 2段配置の最大4ノブと、種類選択・交換ボタンが収まる幅。
            let side_width = GAP * 2.0 + 64.0 * 4.0;
            // NoteとAmpは1行分の同じ高さとし、SensもAHDSRと同じ行へ置く。
            let amp_height = note_height;
            let filter_height = panel_height - note_height - amp_height - GAP * 2.0;
            let effects_body_y = left_top;
            let effect_height = full_height / 3.0;
            panel(
                ui.painter(),
                egui::Rect::from_min_size(
                    egui::pos2(effects_x, left_top),
                    egui::vec2(side_width, full_height),
                ),
                "",
            );
            let effect_kinds = parameters::EFFECT_SLOTS
                .map(|[kind, ..]| parameters::to_plain(kind, selected[kind]) as usize);
            let groups = [
                (
                    "",
                    &[0_usize, 23, 26] as &[usize],
                    &[
                        &[6_usize, 7, 20, 22][..],
                        &[24, 25, 29][..],
                        &[27, 28, 30][..],
                    ][..],
                    [GAP, left_top, oscillator_width, panel_height],
                ),
                (
                    "Amp",
                    &[],
                    &[&[1, 2, 3, 4, 5, 130][..]][..],
                    [center_x, amp_top, filter_width, amp_height],
                ),
                (
                    "Filter",
                    &[9],
                    &[&[10, 11, 19, 131][..], &[14, 15, 16, 17, 18][..]][..],
                    [
                        center_x,
                        amp_top + amp_height + GAP,
                        filter_width,
                        filter_height,
                    ],
                ),
                (
                    "Eff 1",
                    match effect_kinds[0] {
                        3 => &[42, 60][..],
                        4 => &[42, 31][..],
                        _ => &[42][..],
                    },
                    &[match effect_kinds[0] {
                        1 => &[75, 76, 79, 77, 78, 80][..],
                        2 => &[109, 110, 113, 111, 112, 114, 115][..],
                        3 => &[51, 52, 53][..],
                        4 => &[32, 33][..],
                        5 => &[93, 94, 95, 96, 97][..],
                        6 => &[65, 63, 64, 66][..],
                        _ => &[],
                    }][..],
                    [effects_x, effects_body_y, side_width, effect_height],
                ),
                (
                    "Eff 2",
                    match effect_kinds[1] {
                        3 => &[43, 61][..],
                        4 => &[43, 45][..],
                        _ => &[43][..],
                    },
                    &[match effect_kinds[1] {
                        1 => &[81, 82, 85, 83, 84, 86][..],
                        2 => &[116, 117, 120, 118, 119, 121, 122][..],
                        3 => &[54, 55, 56][..],
                        4 => &[46, 47][..],
                        5 => &[98, 99, 100, 101, 102][..],
                        6 => &[69, 67, 68, 70][..],
                        _ => &[],
                    }][..],
                    [
                        effects_x,
                        effects_body_y + effect_height,
                        side_width,
                        effect_height,
                    ],
                ),
                (
                    "Eff 3",
                    match effect_kinds[2] {
                        3 => &[44, 62][..],
                        4 => &[44, 48][..],
                        _ => &[44][..],
                    },
                    &[match effect_kinds[2] {
                        1 => &[87, 88, 91, 89, 90, 92][..],
                        2 => &[123, 124, 127, 125, 126, 128, 129][..],
                        3 => &[57, 58, 59][..],
                        4 => &[49, 50][..],
                        5 => &[103, 104, 105, 106, 107][..],
                        6 => &[73, 71, 72, 74][..],
                        _ => &[],
                    }][..],
                    [
                        effects_x,
                        effects_body_y + effect_height * 2.0,
                        side_width,
                        effect_height,
                    ],
                ),
                (
                    "EQ",
                    &[34],
                    &[&[35, 36, 37, 38, 39, 40, 41][..]][..],
                    [
                        GAP,
                        left_top + panel_height + GAP,
                        effects_x - GAP * 2.0,
                        eq_height,
                    ],
                ),
            ];
            for (title, choices, rows, rect) in groups {
                self.group(ui, title, choices, rows, rect, &selected);
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
            if !parameters::choices(index).is_empty() && parameters::control_kind(index) != 51 {
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
                    114 | 121 | 128 => &[60, 120, 180],
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

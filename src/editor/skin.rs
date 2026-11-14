// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 生成済み素材の伸縮描画とラバーボタン。

use super::{Editor, FONT_SIZE, UI_SCALE, fixed::Layer};
use egui::{Color32, Rect, Shape, pos2, vec2};

pub(super) const BUTTON_TEXT: Color32 = Color32::from_rgb(216, 220, 217);
const METER_BACKGROUND: Color32 = Color32::from_rgb(25, 47, 34);
const METER_ACTIVE: Color32 = Color32::from_rgb(113, 255, 143);

/// 発光面の芯と、暗いガラス面に広がる控えめな光。
fn light(painter: &egui::Painter, rect: Rect, color: Color32) {
    for (radius, alpha) in [(2.0, 12), (1.0, 25)] {
        painter.rect_filled(
            rect.expand(radius),
            2.0,
            Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), alpha),
        );
    }
    painter.rect_filled(rect, 1.0, color);
}

/// 96×40px素材の縁と影を固定し、LCD・ボタンの中央だけを伸縮する。
pub(super) fn surface(texture: egui::TextureId, rect: Rect, tint: Color32) -> Shape {
    let rect = Rect::from_min_max(
        (rect.min.to_vec2() * UI_SCALE).round().to_pos2() / UI_SCALE,
        (rect.max.to_vec2() * UI_SCALE).round().to_pos2() / UI_SCALE,
    )
    .expand(6.0 / UI_SCALE);
    let edge = 12.0 / UI_SCALE;
    let x = [
        rect.left(),
        rect.left() + edge,
        rect.right() - edge,
        rect.right(),
    ];
    let y = [
        rect.top(),
        rect.top() + edge,
        rect.bottom() - edge,
        rect.bottom(),
    ];
    let u = [0.0, 12.0 / 96.0, 84.0 / 96.0, 1.0];
    let v = [0.0, 12.0 / 40.0, 28.0 / 40.0, 1.0];
    let mut mesh = egui::Mesh::with_texture(texture);
    for row in 0..3 {
        for column in 0..3 {
            mesh.add_rect_with_uv(
                Rect::from_min_max(pos2(x[column], y[row]), pos2(x[column + 1], y[row + 1])),
                Rect::from_min_max(pos2(u[column], v[row]), pos2(u[column + 1], v[row + 1])),
                tint,
            );
        }
    }
    Shape::mesh(mesh)
}

impl Editor {
    pub(super) fn draw_meters(&self, painter: &egui::Painter, rect: Rect, voice_count: u32) {
        self.front(painter, || {
            let state = self.state.borrow();
            let inner = rect.shrink(4.0);
            let height = (inner.height() - 6.0) / 2.0;
            let peak = Rect::from_min_size(inner.min, vec2(inner.width(), height));
            let pitch = (inner.width() + 2.0) / 32.0;
            let segment = |voice| {
                Rect::from_min_size(
                    pos2(inner.left() + voice as f32 * pitch, inner.bottom() - height),
                    vec2(pitch - 2.0, height),
                )
            };
            self.fixed(painter, Layer::Controls, |painter| {
                painter.add(surface(state.lcd_texture.id(), rect, Color32::WHITE));
                painter.rect_filled(peak, 1.0, METER_BACKGROUND);
                for voice in 0..32 {
                    painter.rect_filled(segment(voice), 1.0, METER_BACKGROUND);
                }
            });
            let painter = painter.with_clip_rect(inner.expand(1.0));
            for (start_db, end_db, color) in [
                (-60.0, 0.0, METER_ACTIVE),
                (0.0, 12.0, Color32::from_rgb(255, 103, 82)),
            ] {
                let width = peak.width() * (state.peak_db.min(end_db) - start_db).max(0.0) / 72.0;
                if width > 0.0 {
                    light(
                        &painter,
                        Rect::from_min_size(
                            pos2(
                                peak.left() + peak.width() * (start_db + 60.0) / 72.0,
                                peak.top(),
                            ),
                            vec2(width, height),
                        ),
                        color,
                    );
                }
            }
            painter.vline(
                peak.left() + peak.width() * 60.0 / 72.0,
                peak.y_range(),
                egui::Stroke::new(1.0 / UI_SCALE, Color32::from_rgb(72, 109, 86)),
            );
            for voice in 0..voice_count.min(32) {
                light(&painter, segment(voice), METER_ACTIVE);
            }
        });
    }

    pub(super) fn button(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        label: &str,
        indicator: Option<bool>,
    ) -> egui::Response {
        let painter = ui.painter().clone();
        self.front(&painter, || {
            // 操作・フォーカスは既存Buttonへ委ね、外観だけを描く。
            let mut button =
                egui::Button::new(egui::RichText::new(label).color(Color32::TRANSPARENT))
                    .frame(false)
                    .min_size(rect.size());
            if let Some(on) = indicator {
                button = button.selected(on).sense(egui::Sense::click_and_drag());
            }
            let response = ui.put(rect, button);
            let pressed = response.is_pointer_button_down_on();
            let body = |painter: &egui::Painter, rect: Rect, pressed| {
                painter.add(surface(
                    self.state.borrow().button_texture.id(),
                    rect,
                    Color32::from_gray(if pressed { 210 } else { 255 }),
                ));
                painter.text(
                    rect.center() + vec2(0.0, if indicator.is_some() { 4.0 } else { 0.0 }),
                    egui::Align2::CENTER_CENTER,
                    label,
                    egui::FontId::proportional(FONT_SIZE),
                    BUTTON_TEXT,
                );
            };
            self.fixed(&painter, Layer::Controls, |painter| {
                body(painter, response.rect, false)
            });
            let rect = response
                .rect
                .translate(vec2(0.0, f32::from(pressed) / UI_SCALE));
            if pressed {
                // 固定面にある未押下の本体・影を消してから、押し込んだ面を重ねる。
                self.paint_panel_background(
                    &painter,
                    response.rect.expand2(vec2(2.0, 5.0) / UI_SCALE),
                    Color32::WHITE,
                );
                body(&painter, rect, true);
            }
            if let Some(on) = indicator {
                let bar = Rect::from_center_size(
                    pos2(rect.center().x, rect.top() + 7.0),
                    vec2(30.0, 2.5),
                );
                painter.rect_filled(bar.expand(1.2), 2.0, Color32::from_rgb(12, 17, 14));
                if on {
                    light(&painter, bar, METER_ACTIVE);
                } else {
                    painter.rect_filled(bar, 1.0, Color32::from_rgb(33, 54, 40));
                }
            }
            response
        })
    }
}

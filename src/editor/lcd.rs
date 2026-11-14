// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! LCD選択欄と、押下・移動・解放を区別するリスト。開閉状態はeguiが保持する。

use super::{
    CHOICE_HEIGHT, Editor, FONT_SIZE, GAP, KNOB_INK, KNOB_NAMES, OVERLAY_BACKGROUND, edits::Edit,
    fixed::Layer, released_inside,
};
use crate::parameters;
use egui::{Color32, Rect, Shape, Stroke, vec2};

const TEXT: Color32 = Color32::from_rgb(154, 255, 187);

fn popup_style(style: &mut egui::Style) {
    style.spacing.item_spacing.y = 3.0;
    style.spacing.button_padding = vec2(GAP, 2.0);
    style.visuals.selection.bg_fill = Color32::from_rgb(82, 106, 97);
    style.visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(62, 82, 75);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(82, 106, 97);
}

impl Editor {
    pub(super) fn choice(&self, ui: &mut egui::Ui, index: usize, selected: f64) -> Rect {
        if matches!(index, 12 | 31 | 45 | 48 | 60..=62) {
            self.fixed(ui.painter(), Layer::Base, |painter| {
                painter.text(
                    ui.max_rect().center_top() - vec2(0.0, GAP),
                    egui::Align2::CENTER_BOTTOM,
                    KNOB_NAMES[index],
                    egui::FontId::proportional(FONT_SIZE),
                    KNOB_INK,
                );
            });
        }
        let painter = ui.painter().clone();
        self.front(&painter, || {
            let texture = self.state.borrow().lcd_texture.id();
            let names = parameters::choices(index);
            let choice = parameters::to_plain(index, selected) as usize;
            let (_, rect) = ui.allocate_space(vec2(ui.available_width(), CHOICE_HEIGHT));
            let response = ui.interact(
                rect,
                egui::Id::unique(("choice", index)),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::ComboBox, ui.is_enabled(), names[choice])
            });
            self.fixed(&painter, Layer::Controls, |painter| {
                painter.add(super::skin::surface(texture, rect, Color32::WHITE));
            });
            painter.text(
                rect.left_center() + vec2(ui.spacing().button_padding.x, 0.0),
                egui::Align2::LEFT_CENTER,
                names[choice],
                egui::FontId::proportional(FONT_SIZE),
                TEXT,
            );
            let arrow = Rect::from_center_size(
                rect.right_center()
                    - vec2(
                        ui.spacing().button_padding.x + ui.spacing().icon_width / 2.0,
                        0.0,
                    ),
                vec2(9.0, 5.0),
            );
            painter.add(Shape::convex_polygon(
                vec![arrow.left_top(), arrow.right_top(), arrow.center_bottom()],
                TEXT,
                Stroke::NONE,
            ));
            let popup_id = response.id.with("choices");
            // 項目を処理する前に、外側の押下・解放で取り消す。
            // 移動と解放が同じフレームに届いても、外側の押下を選択に変えない。
            let outside = egui::Popup::is_id_open(ui.ctx(), popup_id)
                && ui.ctx().read_response(popup_id).is_some_and(|popup| {
                    ui.input(|input| {
                        input.events.iter().any(|event| matches!(event,
                        egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, .. }
                            if !rect.contains(*pos) && !popup.rect.contains(*pos)
                    ))
                    })
                });
            let command = if outside {
                Some(egui::SetOpenCommand::Bool(false))
            } else {
                released_inside(&response).then_some(egui::SetOpenCommand::Toggle)
            };
            egui::Popup::menu(&response)
                .id(popup_id)
                .open_memory(command)
                .close_behavior(egui::PopupCloseBehavior::IgnoreClicks)
                .width(rect.width())
                .style(popup_style)
                .frame(
                    egui::Frame::new()
                        .fill(OVERLAY_BACKGROUND)
                        .inner_margin(4)
                        .corner_radius(5),
                )
                .show(|ui| {
                    ui.set_min_width(ui.available_width());
                    for (i, &name) in names.iter().enumerate() {
                        let item = ui.add_sized(
                            vec2(ui.available_width(), 24.0),
                            egui::Button::selectable(
                                i == choice,
                                egui::RichText::new(name).color(Color32::WHITE),
                            )
                            .stroke(Stroke::NONE),
                        );
                        if released_inside(&item) {
                            if i != choice {
                                self.edits.push(Edit::Set(
                                    index,
                                    parameters::to_normalized(index, i as f64),
                                ));
                            }
                            ui.close();
                        }
                    }
                });
            rect
        })
    }
}

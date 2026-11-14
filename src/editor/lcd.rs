// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! LCD表示の選択欄。操作とポップアップ管理はeguiのComboBoxを使う。

use super::{Editor, FONT_SIZE, GAP, KNOB_INK, KNOB_NAMES, edits::Edit, fixed::Layer};
use crate::parameters;
use egui::{Color32, Rect, Shape, Stroke, vec2};

const TEXT: Color32 = Color32::from_rgb(154, 255, 187);

fn popup_style(style: &mut egui::Style) {
    style.visuals.window_fill = Color32::from_rgb(16, 32, 22);
    style.visuals.window_stroke = Stroke::new(1.0, Color32::from_rgb(66, 100, 77));
    style.visuals.selection.bg_fill = Color32::from_rgb(27, 73, 46);
    style.visuals.selection.stroke = Stroke::new(1.0, TEXT);
    style.visuals.widgets.hovered.weak_bg_fill = Color32::from_rgb(18, 48, 32);
    style.visuals.widgets.active.weak_bg_fill = Color32::from_rgb(27, 73, 46);
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
            let mut choice = parameters::to_plain(index, selected) as usize;
            let name = names[choice];
            let mut changed = false;
            let rect = ui
                .scope(|ui| {
                    let widgets = &mut ui.visuals_mut().widgets;
                    for widget in [
                        &mut widgets.inactive,
                        &mut widgets.hovered,
                        &mut widgets.active,
                        &mut widgets.open,
                    ] {
                        widget.bg_fill = Color32::TRANSPARENT;
                        widget.weak_bg_fill = Color32::TRANSPARENT;
                        widget.bg_stroke = Stroke::new(1.0, Color32::TRANSPARENT);
                        widget.fg_stroke = Stroke::new(1.0, TEXT);
                        widget.expansion = 0.0;
                    }
                    let painter = ui.painter().clone();
                    let response = egui::ComboBox::from_id_salt(index)
                        .width(ui.available_width())
                        .height(320.0)
                        .icon(|ui, rect, _, _| {
                            let arrow = Rect::from_center_size(rect.center(), vec2(9.0, 5.0));
                            ui.painter().add(Shape::convex_polygon(
                                vec![arrow.left_top(), arrow.right_top(), arrow.center_bottom()],
                                TEXT,
                                Stroke::NONE,
                            ));
                        })
                        .popup_style(popup_style.into())
                        .selected_text(egui::RichText::new(name).color(TEXT))
                        .show_ui(ui, |ui| {
                            for (i, &name) in names.iter().enumerate() {
                                let item = ui.selectable_label(
                                    i == choice,
                                    egui::RichText::new(name).color(TEXT),
                                );
                                if item.clicked() {
                                    choice = i;
                                    changed = true;
                                }
                            }
                        })
                        .response;
                    self.fixed(&painter, Layer::Controls, |painter| {
                        painter.add(super::skin::surface(texture, response.rect, Color32::WHITE));
                    });
                    response.rect
                })
                .inner;
            if changed {
                self.edits.push(Edit::Set(
                    index,
                    parameters::to_normalized(index, choice as f64),
                ));
            }
            rect
        })
    }
}

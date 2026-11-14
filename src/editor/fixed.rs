// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 固定面の記録・更新と、その前後へ重ねる動的表示を分離する。

use super::{Editor, surfaces::Surfaces};
use egui::epaint::ClippedShape;
use egui::{Color32, Context, LayerId, Order, Painter, Rect, Shape, TextureId, pos2};

#[derive(Clone, Copy)]
pub(super) enum Layer {
    Base = 0,
    Controls = 1,
}

fn recording_layer(layer: Layer) -> LayerId {
    LayerId::new(
        Order::Debug,
        egui::Id::unique(("fixed-record", layer as usize)),
    )
}

struct FixedDisplayState {
    /// 寸法・表示倍率・3スロットの種類・Filter/EQのOff状態。
    key: Option<[u32; 8]>,
    /// 配置・固定表示が変わったフレームだけ記録し直す。
    dirty: bool,
    /// 背景と、Off合成/LCD/ボタン面。順序を分けて減光を保つ。
    shapes: [Vec<ClippedShape>; 2],
    /// 固定Controls面の後に描く値・ライト・押下表示。
    front: Vec<ClippedShape>,
    /// フォントアトラスの拡大時にも固定テキストのUVを作り直す。
    font_size: [usize; 2],
}

pub(super) struct FixedDisplay {
    state: FixedDisplayState,
    surfaces: Surfaces,
}

impl FixedDisplay {
    pub(super) fn new(painter: &mut egui_glow::Painter, size: [u32; 2]) -> Self {
        Self {
            state: FixedDisplayState {
                key: None,
                dirty: true,
                shapes: Default::default(),
                front: Vec::new(),
                font_size: [0; 2],
            },
            surfaces: Surfaces::new(painter, size),
        }
    }

    pub(super) fn begin(&mut self, key: [u32; 8]) {
        self.state.dirty = self.state.key != Some(key);
        self.state.key = Some(key);
        self.state.front.clear();
    }

    pub(super) fn image(&self, painter: &Painter, layer: Layer, rect: Rect) {
        let id: TextureId = self.surfaces.texture_ids()[layer as usize];
        // FBOの上下を反転して画面へ戻す。fallbackでは同じIDの位置へ図形を描く。
        painter.image(
            id,
            rect,
            Rect::from_min_max(pos2(0.0, 1.0), pos2(1.0, 0.0)),
            Color32::WHITE,
        );
    }

    pub(super) fn finish(&mut self, painter: &Painter, rect: Rect) {
        if self.state.dirty {
            for layer in [Layer::Base, Layer::Controls] {
                self.state.shapes[layer as usize].clear();
                take_shapes(
                    painter.ctx(),
                    recording_layer(layer),
                    0,
                    &mut self.state.shapes[layer as usize],
                );
            }
        }
        self.image(painter, Layer::Controls, rect);
        painter.ctx().graphics_mut(|graphics| {
            let list = graphics.entry(painter.layer_id());
            for item in self.state.front.drain(..) {
                list.add(item.clip_rect, item.shape);
            }
        });
    }

    pub(super) fn paint(
        &mut self,
        painter: &mut egui_glow::Painter,
        context: &Context,
        size: [u32; 2],
        output: &mut egui::FullOutput,
        primitives: &[egui::ClippedPrimitive],
    ) {
        for (id, deltas) in output.textures_delta.set.drain() {
            for delta in deltas {
                painter.set_texture(id, &delta);
            }
        }
        let font_size = context.fonts(|fonts| fonts.font_image_size());
        if self.state.dirty || self.state.font_size != font_size {
            let fixed = self
                .state
                .shapes
                .each_ref()
                .map(|shapes| context.tessellate(shapes.clone(), output.pixels_per_point));
            self.surfaces
                .rebuild(painter, fixed, size, output.pixels_per_point);
            self.state.font_size = font_size;
        }
        self.surfaces
            .paint(painter, size, output.pixels_per_point, primitives);
        for id in output.textures_delta.free.drain() {
            painter.free_texture(id);
        }
    }

    pub(super) fn destroy(&mut self, painter: &mut egui_glow::Painter) {
        self.surfaces.destroy(painter);
    }
}

/// eguiの描画リストから所有権を移す。操作・ポップアップのレイヤーは変えない。
fn take_shapes(context: &Context, layer: LayerId, from: usize, into: &mut Vec<ClippedShape>) {
    context.graphics_mut(|graphics| {
        let list = graphics.entry(layer);
        for i in from..list.next_idx().0 {
            list.mutate_shape(egui::layers::ShapeIdx(i), |item| {
                into.push(std::mem::replace(
                    item,
                    ClippedShape::new(Rect::NOTHING, Shape::Noop),
                ));
            });
        }
    });
}

impl Editor {
    pub(super) fn engraved_line(&self, painter: &Painter, start: egui::Pos2, end: egui::Pos2) {
        self.fixed(painter, Layer::Base, |painter| {
            super::engraved_line(painter, start, end)
        });
    }
    pub(super) fn fixed(&self, painter: &Painter, layer: Layer, draw: impl FnOnce(&Painter)) {
        if self.fixed.borrow().state.dirty {
            draw(&painter.clone().with_layer_id(recording_layer(layer)));
        }
    }

    pub(super) fn front<T>(&self, painter: &Painter, draw: impl FnOnce() -> T) -> T {
        let from = painter
            .ctx()
            .graphics_mut(|graphics| graphics.entry(painter.layer_id()).next_idx().0);
        let result = draw();
        take_shapes(
            painter.ctx(),
            painter.layer_id(),
            from,
            &mut self.fixed.borrow_mut().state.front,
        );
        result
    }
}

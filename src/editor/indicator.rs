// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! ノブの針と発光弧を、表示値・配置が変わったときだけメッシュ化する。

use super::{KNOB_BODY_SCALE, knob_direction, knob_marks};
use crate::parameters;
use egui::{Color32, Mesh, Painter, Pos2, Rect, Shape, Stroke, epaint};
use std::sync::Arc;

struct IndicatorMesh {
    /// このメッシュが表す整数tick。
    tick: u32,
    /// 物理ピクセルへ位置合わせ済みのノブ中心。
    center: Pos2,
    /// 発光弧2本と針をまとめたメッシュ。
    mesh: Arc<Mesh>,
}

struct KnobIndicatorsState {
    /// パラメーターごとの最後の指示部。非表示のノブも再表示時に再利用する。
    entries: [Option<IndicatorMesh>; parameters::COUNT],
    /// 線のアンチエイリアス処理に用いた表示倍率。
    pixels_per_point: f32,
}

pub(super) struct KnobIndicators {
    state: KnobIndicatorsState,
}

impl Default for KnobIndicators {
    fn default() -> Self {
        Self {
            state: KnobIndicatorsState {
                entries: std::array::from_fn(|_| None),
                pixels_per_point: f32::NAN,
            },
        }
    }
}

impl KnobIndicators {
    pub(super) fn draw(&mut self, painter: &Painter, index: usize, tick: u32, center: Pos2) {
        let context = painter.ctx();
        let pixels_per_point = context.pixels_per_point();
        // 単色の線は固定WHITE_UV=(0, 0)を使い、フォント画像の拡大に依存しない。
        if self.state.pixels_per_point != pixels_per_point {
            self.state.entries.fill_with(|| None);
            self.state.pixels_per_point = pixels_per_point;
        }
        let entry = &mut self.state.entries[index];
        if !entry
            .as_ref()
            .is_some_and(|cached| cached.tick == tick && cached.center == center)
        {
            let maximum = parameters::max_tick(index) as f32;
            let position = tick as f32 / maximum;
            let zero = if parameters::is_volume(index) {
                0.0
            } else {
                knob_marks(index).1 as f32 / maximum
            };
            let points = (0..=32)
                .map(|k| {
                    center
                        + knob_direction(zero + (position - zero) * k as f32 / 32.0)
                            * (19.5 * KNOB_BODY_SCALE)
                })
                .collect::<Vec<_>>();
            let direction = knob_direction(position);
            let shapes = vec![
                Shape::line(
                    points.clone(),
                    Stroke::new(
                        5.0 * KNOB_BODY_SCALE,
                        Color32::from_rgba_unmultiplied(0, 220, 100, 40),
                    ),
                ),
                Shape::line(
                    points,
                    Stroke::new(2.0 * KNOB_BODY_SCALE, Color32::from_rgb(0, 222, 110)),
                ),
                Shape::line_segment(
                    [
                        center + direction * (6.0 * KNOB_BODY_SCALE),
                        center + direction * (15.5 * KNOB_BODY_SCALE),
                    ],
                    Stroke::new(2.0 * KNOB_BODY_SCALE, Color32::BLACK),
                ),
            ];
            // クリップは描画時のPainterへ委ね、再利用する形状を切り落とさない。
            let primitives = context.tessellate(
                vec![epaint::ClippedShape::new(
                    Rect::EVERYTHING,
                    Shape::Vec(shapes),
                )],
                pixels_per_point,
            );
            let mut mesh = Mesh::default();
            for primitive in primitives {
                if let epaint::Primitive::Mesh(part) = primitive.primitive {
                    mesh.append(part);
                }
            }
            *entry = Some(IndicatorMesh {
                tick,
                center,
                mesh: Arc::new(mesh),
            });
        }
        if let Some(cached) = entry {
            painter.add(Shape::Mesh(Arc::clone(&cached.mesh)));
        }
    }
}

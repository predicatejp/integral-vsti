// Copyright (C) 2026 Yusuke Oda
// SPDX-License-Identifier: AGPL-3.0-only

//! 固定の背景・前景をGPU画像へ保持し、対応しないGLでは同じ位置へMeshを描く。

use egui::{ClippedPrimitive, TextureId, epaint::Primitive};
use egui_glow::{Painter, glow};
use glow::HasContext;

struct SurfacesState {
    /// 2枚の固定画像へ付け替えて使う描画先。NoneではMesh描画を使う。
    framebuffer: Option<glow::Framebuffer>,
    /// Painterが所有する画像、またはMesh置換位置を表す予約ID。
    texture_ids: [TextureId; 2],
    /// FBO非対応時にも同じ固定内容を描くための描画命令。
    primitives: [Vec<ClippedPrimitive>; 2],
    /// 現在の画像サイズ。画像のGLハンドルを維持して再確保する。
    size: [u32; 2],
}

pub(super) struct Surfaces {
    state: SurfacesState,
}

impl Surfaces {
    /// 呼出側で、このEditorのGLコンテキストをcurrentにしておく。
    pub(super) fn new(painter: &mut Painter, size: [u32; 2]) -> Self {
        let mut surfaces = Self {
            state: SurfacesState {
                framebuffer: None,
                texture_ids: [TextureId::User(u64::MAX), TextureId::User(u64::MAX - 1)],
                primitives: [Vec::new(), Vec::new()],
                size,
            },
        };
        let gl = painter.gl().clone();
        // baseviewが古いGLへフォールバックした場合は、未対応の関数を呼ばない。
        if (gl.version().major < 3
            && !gl
                .supported_extensions()
                .contains("GL_ARB_framebuffer_object"))
            || size.contains(&0)
            || size
                .iter()
                .any(|&s| s as usize > painter.max_texture_side())
        {
            return surfaces;
        }
        // SAFETY: 呼出側が有効にした専用GL内で生成し、全経路でdefault描画先へ戻す。
        let Ok(framebuffer) = (unsafe { gl.create_framebuffer() }) else {
            return surfaces;
        };
        let mut textures = [None; 2];
        // SAFETY: 作成したFBO/画像だけを操作する。登録前の失敗はこの場で全資源を解放する。
        let complete = unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            let complete = textures.iter_mut().all(|slot| {
                let Ok(texture) = gl.create_texture() else {
                    return false;
                };
                *slot = Some(texture);
                gl.bind_texture(glow::TEXTURE_2D, Some(texture));
                for (parameter, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::NEAREST),
                    (glow::TEXTURE_MAG_FILTER, glow::NEAREST),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    gl.tex_parameter_i32(glow::TEXTURE_2D, parameter, value as i32);
                }
                gl.tex_image_2d(
                    glow::TEXTURE_2D,
                    0,
                    glow::RGBA8 as i32,
                    size[0] as i32,
                    size[1] as i32,
                    0,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelUnpackData::Slice(None),
                );
                gl.framebuffer_texture_2d(
                    glow::FRAMEBUFFER,
                    glow::COLOR_ATTACHMENT0,
                    glow::TEXTURE_2D,
                    Some(texture),
                    0,
                );
                gl.check_framebuffer_status(glow::FRAMEBUFFER) == glow::FRAMEBUFFER_COMPLETE
            });
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            gl.bind_texture(glow::TEXTURE_2D, None);
            if !complete {
                gl.delete_framebuffer(framebuffer);
                for texture in textures.into_iter().flatten() {
                    gl.delete_texture(texture);
                }
            }
            complete
        };
        if complete {
            surfaces.state.framebuffer = Some(framebuffer);
            // 完全性確認後はPainterへ所有権を渡し、こちらでは画像を解放しない。
            surfaces.state.texture_ids =
                textures.map(|texture| painter.register_native_texture(texture.unwrap()));
        }
        surfaces
    }

    /// 各画像は画面全面へUV (0,1)〜(1,0)で描き、FBOの上下方向を反転する。
    pub(super) fn texture_ids(&self) -> [TextureId; 2] {
        self.state.texture_ids
    }

    /// 固定内容が変わったときだけ呼ぶ。primitivesには自身の固定画像を含めない。
    pub(super) fn rebuild(
        &mut self,
        painter: &mut Painter,
        primitives: [Vec<ClippedPrimitive>; 2],
        size: [u32; 2],
        pixels_per_point: f32,
    ) {
        self.state.primitives = primitives;
        let Some(framebuffer) = self.state.framebuffer else {
            return;
        };
        if size.contains(&0)
            || size
                .iter()
                .any(|&s| s as usize > painter.max_texture_side())
        {
            self.destroy(painter);
            return;
        }
        let gl = painter.gl().clone();
        // SAFETY: 呼出側で専用GLがcurrent。画像はPainter内に生存し、描画後にFBOを戻す。
        let complete = unsafe {
            gl.bind_framebuffer(glow::FRAMEBUFFER, Some(framebuffer));
            let complete = self
                .state
                .texture_ids
                .into_iter()
                .enumerate()
                .all(|(i, id)| {
                    let texture = painter.texture(id);
                    if self.state.size != size {
                        gl.bind_texture(glow::TEXTURE_2D, texture);
                        gl.tex_image_2d(
                            glow::TEXTURE_2D,
                            0,
                            glow::RGBA8 as i32,
                            size[0] as i32,
                            size[1] as i32,
                            0,
                            glow::RGBA,
                            glow::UNSIGNED_BYTE,
                            glow::PixelUnpackData::Slice(None),
                        );
                        // 再確保失敗で旧サイズの画像が残ってもFBOは完全になり得る。
                        if gl.get_error() != glow::NO_ERROR {
                            return false;
                        }
                    }
                    gl.framebuffer_texture_2d(
                        glow::FRAMEBUFFER,
                        glow::COLOR_ATTACHMENT0,
                        glow::TEXTURE_2D,
                        texture,
                        0,
                    );
                    if gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE
                    {
                        return false;
                    }
                    painter.clear(size, [0.0; 4]);
                    painter.paint_primitives(size, pixels_per_point, &self.state.primitives[i]);
                    true
                });
            gl.bind_framebuffer(glow::FRAMEBUFFER, None);
            complete
        };
        if complete {
            self.state.size = size;
        } else {
            self.destroy(painter);
        }
    }

    pub(super) fn paint(
        &self,
        painter: &mut Painter,
        size: [u32; 2],
        pixels_per_point: f32,
        primitives: &[ClippedPrimitive],
    ) {
        if self.state.framebuffer.is_some() {
            painter.paint_primitives(size, pixels_per_point, primitives);
            return;
        }
        let mut start = 0;
        for (i, clipped) in primitives.iter().enumerate() {
            if let Primitive::Mesh(mesh) = &clipped.primitive
                && let Some(layer) = self
                    .state
                    .texture_ids
                    .iter()
                    .position(|&id| id == mesh.texture_id)
            {
                painter.paint_primitives(size, pixels_per_point, &primitives[start..i]);
                painter.paint_primitives(size, pixels_per_point, &self.state.primitives[layer]);
                start = i + 1;
            }
        }
        painter.paint_primitives(size, pixels_per_point, &primitives[start..]);
    }

    /// 呼出側で専用GLをcurrentにする。画像の解放はPainter::destroyへ委ねる。
    pub(super) fn destroy(&mut self, painter: &mut Painter) {
        if let Some(framebuffer) = self.state.framebuffer.take() {
            // SAFETY: このコンポーネントが所有し、再度使わないFBOだけを解放する。
            unsafe {
                painter.gl().bind_framebuffer(glow::FRAMEBUFFER, None);
                painter.gl().delete_framebuffer(framebuffer);
            }
        }
    }
}

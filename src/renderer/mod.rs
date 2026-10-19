// mod.rs — Orquestación del renderizado de Realidad Mixta (Tabletop Mixed Reality)
pub mod model;
pub mod sgsr;
pub mod shader;
pub mod warp;

use anyhow::Result;
use glow::HasContext;
use glam::Mat4;
use std::collections::HashMap;
use std::path::Path;
use crate::tabletop::TabletopConfig;

pub struct TabletopRenderer {
    pub warp: warp::WarpPipeline,
    pub models: model::ModelRenderer,
    pub mesh_cache: HashMap<String, model::GpuMesh>,
}

impl TabletopRenderer {
    pub fn new(gl: &glow::Context) -> Result<Self> {
        let warp = warp::WarpPipeline::new(gl)?;
        let models = model::ModelRenderer::new(gl)?;
        Ok(Self {
            warp,
            models,
            mesh_cache: HashMap::new(),
        })
    }

    /// Precarga un modelo GLTF desde disco
    pub fn preload_model<P: AsRef<Path>>(&mut self, gl: &glow::Context, key: &str, path: P) -> Result<()> {
        let mesh = self.models.load_glb(gl, path)?;
        self.mesh_cache.insert(key.to_string(), mesh);
        Ok(())
    }

    /// Renderiza un ojo estereoscópico completo (Left Eye / Right Eye) en OpenXR
    pub fn render_eye(
        &self,
        gl: &glow::Context,
        eye_view_proj: &Mat4,
        table_config: &TabletopConfig,
        table_model: &Mat4,
        scene_texture: glow::Texture,
        aspect: f32,
        plane_scale: f32,
        tex_w: f32,
        tex_h: f32,
        entities: &[(String, Vec<Mat4>)],
    ) {
        unsafe {
            // 1. Configurar estado GL para renderizado 3D
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LEQUAL);
            gl.enable(glow::BLEND);
            gl.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);

            // 2. Renderizar suelo de la fábrica (Warp) sobre la mesa física
            self.warp.render_tabletop_ground(
                gl,
                scene_texture,
                eye_view_proj,
                table_config,
                table_model,
                aspect,
                plane_scale,
                tex_w,
                tex_h,
            );

            // 3. Renderizar mallas 3D de máquinas sobre las casillas de la mesa
            for (key, transforms) in entities {
                if let Some(mesh) = self.mesh_cache.get(key) {
                    self.models.draw_instances(
                        gl,
                        mesh,
                        transforms,
                        eye_view_proj,
                        table_config,
                        table_model,
                    );
                }
            }
        }
    }
}

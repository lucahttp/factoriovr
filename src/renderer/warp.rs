// warp.rs — Pipeline de deformación del plano de suelo sobre la mesa física
use anyhow::Result;
use glow::HasContext;
use glam::{Mat4, Vec4};
use crate::renderer::shader::ShaderProgram;
use crate::tabletop::TabletopConfig;

pub struct WarpPipeline {
    shader: ShaderProgram,
    vao: glow::VertexArray,
}

impl WarpPipeline {
    pub fn new(gl: &glow::Context) -> Result<Self> {
        let vert_src = include_str!("../../shaders/warp.vert");
        let frag_src = include_str!("../../shaders/warp.frag");
        let shader = ShaderProgram::new(gl, vert_src, frag_src)?;

        unsafe {
            let vao = gl.create_vertex_array().map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Self { shader, vao })
        }
    }

    /// Renderiza el suelo deformado sobre el plano de la mesa
    pub fn render_tabletop_ground(
        &self,
        gl: &glow::Context,
        scene_texture: glow::Texture,
        view_proj: &Mat4,
        table_config: &TabletopConfig,
        table_model: &Mat4,
        aspect: f32,
        plane_scale: f32,
        tex_w: f32,
        tex_h: f32,
    ) {
        unsafe {
            self.shader.bind(gl);

            // 1. Textura del mundo de Factorio
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(scene_texture));
            self.shader.set_int(gl, "u_scene_tex", 0);

            // 2. Matrices y parámetros de vista
            self.shader.set_mat4(gl, "u_view_proj", view_proj);
            self.shader.set_mat4(gl, "u_table_model", table_model);
            self.shader.set_float(gl, "u_aspect", aspect);
            self.shader.set_float(gl, "u_plane_scale", plane_scale);
            self.shader.set_float(gl, "u_tex_w", tex_w);
            self.shader.set_float(gl, "u_tex_h", tex_h);

            // 3. Límites físicos de la mesa (Stencil Box)
            let half_w = table_config.table_dimensions.x * 0.5;
            let half_d = table_config.table_dimensions.y * 0.5;
            let bounds = Vec4::new(
                table_config.anchor_position.x - half_w,
                table_config.anchor_position.z - half_d,
                table_config.anchor_position.x + half_w,
                table_config.anchor_position.z + half_d,
            );
            self.shader.set_vec4(gl, "u_table_bounds", &bounds);
            self.shader.set_vec3(gl, "u_table_anchor", &table_config.anchor_position);

            // 4. Dibujar plano quad
            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            gl.bind_vertex_array(None);
        }
    }
}

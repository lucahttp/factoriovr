// sgsr.rs — Integración de Snapdragon Game Super Resolution (Qualcomm SGSR) para Pico 4 Ultra (Adreno 740)
//
// Permite renderizar Factorio a menor resolución interna (ej. 1080p o 1440p) y reescalarlo
// mediante hardware Adreno a la resolución nativa 4K (2160x2160 por ojo) de la Pico 4 Ultra
// con latencia < 0.8ms y sin artefactos de desenfoque.

use anyhow::Result;
use glow::HasContext;
use glam::Vec4;
use crate::renderer::shader::ShaderProgram;

pub struct SgsrUpscaler {
    shader: ShaderProgram,
    vao: glow::VertexArray,
    pub input_size: (u32, u32),
    pub output_size: (u32, u32),
}

impl SgsrUpscaler {
    pub fn new(gl: &glow::Context, input_size: (u32, u32), output_size: (u32, u32)) -> Result<Self> {
        let vert_src = include_str!("../../shaders/sky.vert"); // Reutiliza el quad a pantalla completa
        let frag_src = include_str!("../../shaders/sgsr/sgsr1.frag");
        let shader = ShaderProgram::new(gl, vert_src, frag_src)?;

        unsafe {
            let vao = gl.create_vertex_array().map_err(|e| anyhow::anyhow!("{e}"))?;
            Ok(Self {
                shader,
                vao,
                input_size,
                output_size,
            })
        }
    }

    /// Ejecuta el reescalado super-resolución de Qualcomm en un solo pase de fragment shader
    pub fn upscale(
        &self,
        gl: &glow::Context,
        input_texture: glow::Texture,
    ) {
        unsafe {
            self.shader.bind(gl);

            // Textura de entrada (FBO de Factorio a menor resolución)
            gl.active_texture(glow::TEXTURE0);
            gl.bind_texture(glow::TEXTURE_2D, Some(input_texture));
            self.shader.set_int(gl, "ps0", 0);

            // ViewportInfo de Qualcomm: (width, height, 1.0 / width, 1.0 / height)
            let in_w = self.input_size.0 as f32;
            let in_h = self.input_size.1 as f32;
            let viewport_info = Vec4::new(in_w, in_h, 1.0 / in_w, 1.0 / in_h);
            self.shader.set_vec4(gl, "ViewportInfo[0]", &viewport_info);

            // Configurar viewport de salida para pantalla completa en las lentes de la Pico 4 Ultra
            gl.viewport(0, 0, self.output_size.0 as i32, self.output_size.1 as i32);

            gl.bind_vertex_array(Some(self.vao));
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
            gl.bind_vertex_array(None);
        }
    }
}

//! sgsr.rs — Snapdragon Game Super Resolution (Qualcomm) en OpenGL ES 3.2.

// En la edicion 2021 el cuerpo de una `unsafe fn` ya es un bloque
// `unsafe`: los bloques de aqui son redundantes (y en la 2024 harian falta).
#![allow(unused_unsafe)]
//!
//! La pantalla de la Pico 4 Ultra es de 2160x2160 por ojo: renderizar a
//! esa resolucion en VR es caro.  SGSR renderiza la escena a menos
//! resolucion (por ejemplo 1080x1080) y luego reescala con el filtro
//! espacial de Qualcomm, que queda mucho mas nitido que un bicubico.
//!
//! COMO SE USA (desde el motor de VR en C++):
//!
//! 1. `xfactorvr_sgsr_preparar(w_in, h_in, w_out, h_out)` crea el
//!    framebuffer y la textura de entrada.
//! 2. El motor renderiza la escena EN ese framebuffer:
//!    `glBindFramebuffer(GL_FRAMEBUFFER, xfactorvr_sgsr_fbo())`.
//! 3. El motor vuelve a su framebuffer de siempre (el swapchain) y llama
//!    `xfactorvr_sgsr_escalar()`: se dibuja un triangulo de pantalla
//!    completa con el shader de SGSR y se escribe a 2160x2160.
//!
//! Los shaders de SGSR 2 (los de historico y movimiento) son GLSL de
//! Vulkan (`layout(set=..., binding=...)`) y necesitan texturas de
//! velocidad/profundidad que aqui no existen, asi que se usa el SGSR 1
//! (espacial, un solo pase), que es GLSL ES puro y funciona en el GLES 3.2
//! del visor.  El shader se subio de `#version 300 es` a `#version 320 es`
//! porque usa `textureGather` (GLSL ES 3.1+).

use crate::renderer::gl::*;
use crate::renderer::shader::ShaderProgram;
use anyhow::{anyhow, Result};

/// Pipeline de upscaling SGSR.
pub struct SgsrUpscaler {
    shader: ShaderProgram,
    /// VAO vacio para el triangulo de pantalla completa.
    vao: GLuint,
    /// Textura de entrada (la escena renderizada a baja resolucion).
    textura: GLuint,
    /// Renderbuffer de profundidad de la entrada.
    #[allow(dead_code)]
    profundidad: GLuint,
    /// Framebuffer donde se renderiza la escena a baja resolucion.
    fbo: GLuint,
    /// Tamano de la entrada (la resolucion a la que se renderiza).
    pub input_size: (u32, u32),
    /// Tamano de la salida (la resolucion de la pantalla).
    pub output_size: (u32, u32),
}

impl SgsrUpscaler {
    /// Prepara el pipeline para una entrada y una salida dadas.
    pub fn new(gl: &Gl, input_size: (u32, u32), output_size: (u32, u32)) -> Result<Self> {
        if input_size.0 == 0 || input_size.1 == 0 || output_size.0 == 0 || output_size.1 == 0 {
            return Err(anyhow!("tamaño de SGSR invalido"));
        }
        let shader = ShaderProgram::new(
            gl,
            &crate::renderer::shader::vert_fullscreen(),
            &crate::renderer::shader::frag_sgsr1(),
        )?;

        unsafe {
            // Textura de color de la entrada.
            let mut tex: GLuint = 0;
            (gl.GenTextures)(1, &mut tex);
            (gl.BindTexture)(TEXTURE_2D, tex);
            (gl.TexImage2D)(
                TEXTURE_2D,
                0,
                RGBA8 as GLint,
                input_size.0 as GLsizei,
                input_size.1 as GLsizei,
                0,
                RGBA,
                UNSIGNED_BYTE,
                std::ptr::null(),
            );
            (gl.TexParameteri)(TEXTURE_2D, TEXTURE_MIN_FILTER, LINEAR as GLint);
            (gl.TexParameteri)(TEXTURE_2D, TEXTURE_MAG_FILTER, LINEAR as GLint);
            (gl.TexParameteri)(TEXTURE_2D, TEXTURE_WRAP_S, CLAMP_TO_EDGE as GLint);
            (gl.TexParameteri)(TEXTURE_2D, TEXTURE_WRAP_T, CLAMP_TO_EDGE as GLint);

            // Profundidad.
            let mut rbo: GLuint = 0;
            (gl.GenRenderbuffers)(1, &mut rbo);
            (gl.BindRenderbuffer)(RENDERBUFFER, rbo);
            (gl.RenderbufferStorage)(
                RENDERBUFFER,
                DEPTH_COMPONENT16,
                input_size.0 as GLsizei,
                input_size.1 as GLsizei,
            );

            // Framebuffer.
            let mut fbo: GLuint = 0;
            (gl.GenFramebuffers)(1, &mut fbo);
            (gl.BindFramebuffer)(FRAMEBUFFER, fbo);
            (gl.FramebufferTexture2D)(FRAMEBUFFER, COLOR_ATTACHMENT0, TEXTURE_2D, tex, 0);
            (gl.FramebufferRenderbuffer)(FRAMEBUFFER, DEPTH_ATTACHMENT, RENDERBUFFER, rbo);
            let estado = (gl.CheckFramebufferStatus)(FRAMEBUFFER);
            (gl.BindFramebuffer)(FRAMEBUFFER, 0);
            if estado != FRAMEBUFFER_COMPLETE {
                return Err(anyhow!(
                    "el framebuffer de SGSR no esta completo (0x{estado:x})"
                ));
            }

            let mut vao: GLuint = 0;
            (gl.GenVertexArrays)(1, &mut vao);

            Ok(SgsrUpscaler {
                shader,
                vao,
                textura: tex,
                profundidad: rbo,
                fbo,
                input_size,
                output_size,
            })
        }
    }

    /// Nombre GL del framebuffer de entrada (donde se renderiza la escena).
    pub fn framebuffer(&self) -> GLuint {
        self.fbo
    }

    /// Nombre GL de la textura de entrada.
    pub fn textura(&self) -> GLuint {
        self.textura
    }

    /// Reescala la entrada a la resolucion de salida, en el framebuffer que
    /// este ligado ahora mismo.
    ///
    /// Devuelve true si se ha dibujado.
    pub fn escalar(&self, gl: &Gl) -> bool {
        unsafe {
            self.shader.bind(gl);
            (gl.ActiveTexture)(TEXTURE0);
            (gl.BindTexture)(TEXTURE_2D, self.textura);
            self.shader.set_i(gl, "ps0", 0);
            // ViewportInfo: (ancho, alto, 1/ancho, 1/alto)
            let w = self.input_size.0 as f32;
            let h = self.input_size.1 as f32;
            self.shader
                .set_v4(gl, "ViewportInfo[0]", [w, h, 1.0 / w, 1.0 / h]);

            (gl.Disable)(DEPTH_TEST);
            (gl.Disable)(BLEND);
            (gl.Viewport)(0, 0, self.output_size.0 as GLsizei, self.output_size.1 as GLsizei);
            (gl.BindVertexArray)(self.vao);
            (gl.DrawArrays)(TRIANGLES, 0, 3);
            (gl.BindVertexArray)(0);
            (gl.UseProgram)(0);
        }
        true
    }
}

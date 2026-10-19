// shader.rs — Compilación, enlazado y gestión de shaders OpenGL ES 3.2 / Core
use anyhow::{Context, Result, bail};
use glow::HasContext;

pub struct ShaderProgram {
    pub program: glow::Program,
}

impl ShaderProgram {
    pub fn new(gl: &glow::Context, vert_source: &str, frag_source: &str) -> Result<Self> {
        unsafe {
            let program = gl.create_program().map_err(|e| anyhow::anyhow!("{e}"))?;

            let vert_shader = Self::compile_stage(gl, glow::VERTEX_SHADER, vert_source)
                .context("Error compilando Vertex Shader")?;
            let frag_shader = Self::compile_stage(gl, glow::FRAGMENT_SHADER, frag_source)
                .context("Error compilando Fragment Shader")?;

            gl.attach_shader(program, vert_shader);
            gl.attach_shader(program, frag_shader);
            gl.link_program(program);

            if !gl.get_program_link_status(program) {
                let log = gl.get_program_info_log(program);
                gl.delete_shader(vert_shader);
                gl.delete_shader(frag_shader);
                gl.delete_program(program);
                bail!("Error enlazando Shader Program: {log}");
            }

            // Los shaders individuales ya no se necesitan una vez enlazado el programa
            gl.detach_shader(program, vert_shader);
            gl.detach_shader(program, frag_shader);
            gl.delete_shader(vert_shader);
            gl.delete_shader(frag_shader);

            Ok(Self { program })
        }
    }

    unsafe fn compile_stage(gl: &glow::Context, shader_type: u32, source: &str) -> Result<glow::Shader> {
        let shader = gl.create_shader(shader_type).map_err(|e| anyhow::anyhow!("{e}"))?;
        gl.shader_source(shader, source);
        gl.compile_shader(shader);

        if !gl.get_shader_compile_status(shader) {
            let log = gl.get_shader_info_log(shader);
            gl.delete_shader(shader);
            bail!("Error de compilación en etapa {shader_type}: {log}");
        }

        Ok(shader)
    }

    pub fn bind(&self, gl: &glow::Context) {
        unsafe {
            gl.use_program(Some(self.program));
        }
    }

    pub fn set_mat4(&self, gl: &glow::Context, name: &str, mat: &glam::Mat4) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(self.program, name) {
                gl.uniform_matrix_4_f32_slice(Some(&loc), false, &mat.to_cols_array());
            }
        }
    }

    pub fn set_vec4(&self, gl: &glow::Context, name: &str, vec: &glam::Vec4) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(self.program, name) {
                gl.uniform_4_f32(Some(&loc), vec.x, vec.y, vec.z, vec.w);
            }
        }
    }

    pub fn set_vec3(&self, gl: &glow::Context, name: &str, vec: &glam::Vec3) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(self.program, name) {
                gl.uniform_3_f32(Some(&loc), vec.x, vec.y, vec.z);
            }
        }
    }

    pub fn set_float(&self, gl: &glow::Context, name: &str, val: f32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(self.program, name) {
                gl.uniform_1_f32(Some(&loc), val);
            }
        }
    }

    pub fn set_int(&self, gl: &glow::Context, name: &str, val: i32) {
        unsafe {
            if let Some(loc) = gl.get_uniform_location(self.program, name) {
                gl.uniform_1_i32(Some(&loc), val);
            }
        }
    }
}

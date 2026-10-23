//! shader.rs — Compila y usa los programas de GLSL del visor.

// En la edicion 2021 el cuerpo de una `unsafe fn` ya es un bloque
// `unsafe`: los bloques de aqui son redundantes (y en la 2024 harian falta).
#![allow(unused_unsafe)]
//!
//! Los shaders viven en `android/src/main/assets/shaders/` (van dentro del
//! APK).  En tiempo de ejecucion se buscan en el directorio de datos de la
//! app (donde el motor copia los assets, como hace con el atlas y los
//! modelos) y, si no estan ahi, se usa la COPIA EMBEBIDA en el binario:
//! asi el APK nunca se queda sin shaders aunque el motor no los haya
//! copiado todavia.
//!
//! Se GLSL plano (uniformes por nombre), nada de Uniform Blocks: el visor
//! usa OpenGL ES 3.2 y asi es mucho mas simple de enlazar.

use crate::renderer::gl::*;
use anyhow::{anyhow, Result};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Copias embebidas de los shaders (si no se encuentran en disco).
const VERT_MODELO: &str = include_str!("../../android/src/main/assets/shaders/modelo_visor.vert");
const FRAG_MODELO: &str = include_str!("../../android/src/main/assets/shaders/modelo_visor.frag");
const VERT_FULLSCREEN: &str = include_str!("../../android/src/main/assets/shaders/fullscreen.vert");
const FRAG_SGSR1: &str = include_str!("../../android/src/main/assets/shaders/sgsr/sgsr1.frag");

/// Directorio donde se leen los shaders en tiempo de ejecucion.
///
/// En el APK los assets no se leen del APK: el motor los copia a su
/// directorio de datos.  Se buscan los mismos sitios que los modelos.
pub fn dir_shaders() -> PathBuf {
    if let Ok(d) = std::env::var("XFACTORVR_SHADERS_DIR") {
        return PathBuf::from(d);
    }
    const CANDIDATOS: &[&str] = &[
        "/sdcard/Android/data/com.xfactorvr.tabletop/files/shaders",
        "/storage/emulated/0/Android/data/com.xfactorvr.tabletop/files/shaders",
        "/data/data/com.xfactorvr.tabletop/files/shaders",
        "/data/user/0/com.xfactorvr.tabletop/files/shaders",
        "/sdcard/XFactorVR/assets/shaders",
        "/storage/emulated/0/XFactorVR/assets/shaders",
        "/data/local/tmp/XFactorVR/assets/shaders",
        "android/src/main/assets/shaders",
    ];
    for c in CANDIDATOS {
        let p = Path::new(c);
        if p.join("modelo_visor.vert").exists() {
            return p.to_path_buf();
        }
    }
    PathBuf::from(CANDIDATOS[0])
}

/// Lee un shader de disco; si no esta, usa la copia embebida.
pub fn leer_shader(nombre: &str, embebido: &str) -> String {
    let ruta = dir_shaders().join(nombre);
    match std::fs::read_to_string(&ruta) {
        Ok(s) => s,
        Err(_) => embebido.to_string(),
    }
}

/// Vertex shader del modelo 3D.
pub fn vert_modelo() -> String {
    leer_shader("modelo_visor.vert", VERT_MODELO)
}

/// Fragment shader del modelo 3D.
pub fn frag_modelo() -> String {
    leer_shader("modelo_visor.frag", FRAG_MODELO)
}

/// Vertex shader de pantalla completa (SGSR).
pub fn vert_fullscreen() -> String {
    leer_shader("fullscreen.vert", VERT_FULLSCREEN)
}

/// Fragment shader SGSR 1 (upscaling espacial de Qualcomm).
pub fn frag_sgsr1() -> String {
    leer_shader("sgsr/sgsr1.frag", FRAG_SGSR1)
}

/// Programa de GLSL compilado, con la cache de posiciones de uniformes.
pub struct ShaderProgram {
    pub program: GLuint,
    uniforms: RefCell<HashMap<String, GLint>>,
}

impl ShaderProgram {
    /// Compila y enlaza un programa.
    pub fn new(gl: &Gl, vert_src: &str, frag_src: &str) -> Result<Self> {
        unsafe {
            let vs = compilar(gl, VERTEX_SHADER, vert_src)?;
            let fs = compilar(gl, FRAGMENT_SHADER, frag_src)?;
            let program = (gl.CreateProgram)();
            if program == 0 {
                return Err(anyhow!("glCreateProgram devolvio 0"));
            }
            (gl.AttachShader)(program, vs);
            (gl.AttachShader)(program, fs);
            (gl.LinkProgram)(program);
            let mut ok: GLint = 0;
            (gl.GetProgramiv)(program, LINK_STATUS, &mut ok);
            if ok == 0 {
                let log = log_programo(gl, program);
                (gl.DeleteProgram)(program);
                (gl.DeleteShader)(vs);
                (gl.DeleteShader)(fs);
                return Err(anyhow!("no se pudo enlazar el programa: {log}"));
            }
            (gl.DeleteShader)(vs);
            (gl.DeleteShader)(fs);
            Ok(ShaderProgram {
                program,
                uniforms: RefCell::new(HashMap::new()),
            })
        }
    }

    /// Activa el programa.
    pub fn bind(&self, gl: &Gl) {
        unsafe {
            (gl.UseProgram)(self.program);
        }
    }

    /// Posicion de un uniforme (cacheada; -1 si el shader no lo tiene).
    pub fn location(&self, gl: &Gl, nombre: &str) -> GLint {
        if let Some(v) = self.uniforms.borrow().get(nombre) {
            return *v;
        }
        let loc = unsafe {
            let c = std::ffi::CString::new(nombre).unwrap_or_default();
            (gl.GetUniformLocation)(self.program, c.as_ptr() as *const GLchar)
        };
        self.uniforms.borrow_mut().insert(nombre.to_string(), loc);
        loc
    }

    pub fn set_i(&self, gl: &Gl, nombre: &str, v: i32) {
        let l = self.location(gl, nombre);
        if l >= 0 {
            unsafe { (gl.Uniform1i)(l, v) };
        }
    }

    pub fn set_f(&self, gl: &Gl, nombre: &str, v: f32) {
        let l = self.location(gl, nombre);
        if l >= 0 {
            unsafe { (gl.Uniform1f)(l, v) };
        }
    }

    pub fn set_v3(&self, gl: &Gl, nombre: &str, v: [f32; 3]) {
        let l = self.location(gl, nombre);
        if l >= 0 {
            unsafe { (gl.Uniform3f)(l, v[0], v[1], v[2]) };
        }
    }

    pub fn set_v4(&self, gl: &Gl, nombre: &str, v: [f32; 4]) {
        let l = self.location(gl, nombre);
        if l >= 0 {
            unsafe { (gl.Uniform4f)(l, v[0], v[1], v[2], v[3]) };
        }
    }

    /// Matriz 4x4 en orden COLUMN-MAJOR (como la espera GL).
    pub fn set_mat4(&self, gl: &Gl, nombre: &str, m: &glam::Mat4) {
        let l = self.location(gl, nombre);
        if l >= 0 {
            let c = m.to_cols_array();
            unsafe { (gl.UniformMatrix4fv)(l, 1, FALSE_, c.as_ptr()) };
        }
    }
}

/// Compila un shader y devuelve su id.
unsafe fn compilar(gl: &Gl, tipo: GLenum, src: &str) -> Result<GLuint> {
    let id = (gl.CreateShader)(tipo);
    if id == 0 {
        return Err(anyhow!("glCreateShader devolvio 0"));
    }
    let bytes = src.as_bytes();
    let puntero = bytes.as_ptr() as *const GLchar;
    let len: GLint = bytes.len() as GLint;
    (gl.ShaderSource)(id, 1, &puntero, &len);
    (gl.CompileShader)(id);
    let mut ok: GLint = 0;
    (gl.GetShaderiv)(id, COMPILE_STATUS, &mut ok);
    if ok == 0 {
        let log = log_shader(gl, id);
        (gl.DeleteShader)(id);
        let tipo_txt = if tipo == VERTEX_SHADER { "vertex" } else { "fragment" };
        return Err(anyhow!("no se pudo compilar el shader {tipo_txt}: {log}"));
    }
    Ok(id)
}

unsafe fn log_shader(gl: &Gl, id: GLuint) -> String {
    let mut len: GLint = 0;
    (gl.GetShaderiv)(id, INFO_LOG_LENGTH, &mut len);
    if len <= 0 {
        return "(sin log)".to_string();
    }
    let mut buf = vec![0u8; len as usize];
    (gl.GetShaderInfoLog)(id, len, std::ptr::null_mut(), buf.as_mut_ptr() as *mut GLchar);
    String::from_utf8_lossy(&buf).to_string()
}

unsafe fn log_programo(gl: &Gl, id: GLuint) -> String {
    let mut len: GLint = 0;
    (gl.GetProgramiv)(id, INFO_LOG_LENGTH, &mut len);
    if len <= 0 {
        return "(sin log)".to_string();
    }
    let mut buf = vec![0u8; len as usize];
    (gl.GetProgramInfoLog)(id, len, std::ptr::null_mut(), buf.as_mut_ptr() as *mut GLchar);
    String::from_utf8_lossy(&buf).to_string()
}

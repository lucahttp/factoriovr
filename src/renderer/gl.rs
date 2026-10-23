//! Enlace mínimo de OpenGL ES 3.2 para el renderer 3D del VISOR.

// En la edicion 2021 el cuerpo de una `unsafe fn` ya es un bloque
// `unsafe`: los bloques de aqui son redundantes (y en la 2024 harian falta).
#![allow(unused_unsafe)]
//!
//! El visor (el motor de VR en C++) tiene el contexto EGL de GLES 3.2
//! corriente cuando llama a nuestras FFI, asi que aqui NO se crea contexto:
//! solo se cargan los punteros a las funciones de `libGLESv2.so` con
//! `dlsym` y se llama a traves de ellos.
//!
//! Por que `dlsym` y no `#[link(name = "GLESv2")]`: el NDK de Android NO
//! trae la libreria de stubs `libGLESv2.so` en el sysroot (solo libc, libm,
//! libdl...), asi que enlazarla en tiempo de compilacion haria fallar el
//! build del APK.  Cargandola en tiempo de ejecucion no hace falta nada.
//!
//! En el host (macOS, donde se corren los tests) no hay GL: `cargar()`
//! devuelve error y el renderer simplemente no dibuja.  Los tests del modulo
//! son de CPU (geometria, matrices, instancias), no de GPU.

use anyhow::{anyhow, Result};

// ---------------------------------------------------------------------------
// Tipos y constantes
// ---------------------------------------------------------------------------

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLsizeiptr = isize;
pub type GLintptr = isize;

pub const FALSE_: GLboolean = 0;
pub const TRUE_: GLboolean = 1;

pub const DEPTH_TEST: GLenum = 0x0B71;
pub const DEPTH_WRITEMASK: GLenum = 0x0B72;
pub const BLEND: GLenum = 0x0BE2;
pub const CULL_FACE: GLenum = 0x0B44;
pub const SCISSOR_TEST: GLenum = 0x0C11;
pub const LEQUAL: GLenum = 0x0203;
pub const SRC_ALPHA: GLenum = 0x0302;
pub const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;

pub const ARRAY_BUFFER: GLenum = 0x8892;
pub const ELEMENT_ARRAY_BUFFER: GLenum = 0x8893;
pub const STATIC_DRAW: GLenum = 0x88E4;
pub const DYNAMIC_DRAW: GLenum = 0x88E8;

pub const FLOAT: GLenum = 0x1406;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const UNSIGNED_INT: GLenum = 0x1405;
pub const TRIANGLES: GLenum = 0x0004;

pub const VERTEX_SHADER: GLenum = 0x8B31;
pub const FRAGMENT_SHADER: GLenum = 0x8B30;
pub const COMPILE_STATUS: GLenum = 0x8B81;
pub const LINK_STATUS: GLenum = 0x8B82;
pub const INFO_LOG_LENGTH: GLenum = 0x8B84;

pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const TEXTURE0: GLenum = 0x84C0;
pub const RGBA: GLenum = 0x1908;
pub const RGBA8: GLenum = 0x8058;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const TEXTURE_WRAP_S: GLenum = 0x2802;
pub const TEXTURE_WRAP_T: GLenum = 0x2803;
pub const NEAREST: GLenum = 0x2600;
pub const LINEAR: GLenum = 0x2601;
pub const CLAMP_TO_EDGE: GLenum = 0x812F;

pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const RENDERBUFFER: GLenum = 0x8D41;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const DEPTH_ATTACHMENT: GLenum = 0x8D00;
pub const DEPTH_COMPONENT16: GLenum = 0x81A5;
pub const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;

pub const NO_ERROR: GLenum = 0;

// ---------------------------------------------------------------------------
// El contexto
// ---------------------------------------------------------------------------

/// Punteros a las funciones de GL que usa el renderer.
///
/// Es una struct plana (no un trait como en `glow`): asi no hace falta
/// implementar el API completo de OpenGL, solo lo que se usa.
#[allow(non_snake_case)]
#[derive(Clone, Copy)]
pub struct Gl {
    // shaders y programas
    pub CreateShader: unsafe extern "C" fn(GLenum) -> GLuint,
    pub ShaderSource:
        unsafe extern "C" fn(GLuint, GLsizei, *const *const GLchar, *const GLint),
    pub CompileShader: unsafe extern "C" fn(GLuint),
    pub GetShaderiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    pub GetShaderInfoLog:
        unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
    pub DeleteShader: unsafe extern "C" fn(GLuint),
    pub CreateProgram: unsafe extern "C" fn() -> GLuint,
    pub AttachShader: unsafe extern "C" fn(GLuint, GLuint),
    pub LinkProgram: unsafe extern "C" fn(GLuint),
    pub GetProgramiv: unsafe extern "C" fn(GLuint, GLenum, *mut GLint),
    pub GetProgramInfoLog:
        unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
    pub UseProgram: unsafe extern "C" fn(GLuint),
    pub DeleteProgram: unsafe extern "C" fn(GLuint),
    pub GetUniformLocation: unsafe extern "C" fn(GLuint, *const GLchar) -> GLint,
    pub Uniform1i: unsafe extern "C" fn(GLint, GLint),
    pub Uniform1f: unsafe extern "C" fn(GLint, GLfloat),
    pub Uniform3f: unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat),
    pub Uniform4f: unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat, GLfloat),
    pub UniformMatrix4fv: unsafe extern "C" fn(GLint, GLsizei, GLboolean, *const GLfloat),

    // buffers y vertex arrays
    pub GenBuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub BindBuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub BufferData:
        unsafe extern "C" fn(GLenum, GLsizeiptr, *const std::os::raw::c_void, GLenum),
    pub BufferSubData:
        unsafe extern "C" fn(GLenum, GLintptr, GLsizeiptr, *const std::os::raw::c_void),
    pub DeleteBuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub GenVertexArrays: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub BindVertexArray: unsafe extern "C" fn(GLuint),
    pub DeleteVertexArrays: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub EnableVertexAttribArray: unsafe extern "C" fn(GLuint),
    pub VertexAttribPointer:
        unsafe extern "C" fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const std::os::raw::c_void),
    pub VertexAttribDivisor: unsafe extern "C" fn(GLuint, GLuint),

    // estado
    pub Enable: unsafe extern "C" fn(GLenum),
    pub Disable: unsafe extern "C" fn(GLenum),
    pub DepthFunc: unsafe extern "C" fn(GLenum),
    pub DepthMask: unsafe extern "C" fn(GLboolean),
    pub BlendFunc: unsafe extern "C" fn(GLenum, GLenum),
    pub CullFace: unsafe extern "C" fn(GLenum),
    pub GetError: unsafe extern "C" fn() -> GLenum,
    pub GetIntegerv: unsafe extern "C" fn(GLenum, *mut GLint),
    pub GetBooleanv: unsafe extern "C" fn(GLenum, *mut GLboolean),
    pub IsEnabled: unsafe extern "C" fn(GLenum) -> GLboolean,
    pub Viewport: unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei),

    // dibujo
    pub DrawArrays: unsafe extern "C" fn(GLenum, GLint, GLsizei),
    pub DrawArraysInstanced: unsafe extern "C" fn(GLenum, GLint, GLsizei, GLsizei),
    pub DrawElements: unsafe extern "C" fn(GLenum, GLsizei, GLenum, *const std::os::raw::c_void),
    pub DrawElementsInstanced:
        unsafe extern "C" fn(GLenum, GLsizei, GLenum, *const std::os::raw::c_void, GLsizei),

    // texturas y framebuffers (SGSR)
    pub GenTextures: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub BindTexture: unsafe extern "C" fn(GLenum, GLuint),
    pub TexImage2D: unsafe extern "C" fn(
        GLenum,
        GLint,
        GLint,
        GLsizei,
        GLsizei,
        GLint,
        GLenum,
        GLenum,
        *const std::os::raw::c_void,
    ),
    pub TexParameteri: unsafe extern "C" fn(GLenum, GLenum, GLint),
    pub ActiveTexture: unsafe extern "C" fn(GLenum),
    pub GenFramebuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub BindFramebuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub DeleteFramebuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub FramebufferTexture2D: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
    pub CheckFramebufferStatus: unsafe extern "C" fn(GLenum) -> GLenum,
    pub GenRenderbuffers: unsafe extern "C" fn(GLsizei, *mut GLuint),
    pub BindRenderbuffer: unsafe extern "C" fn(GLenum, GLuint),
    pub DeleteRenderbuffers: unsafe extern "C" fn(GLsizei, *const GLuint),
    pub RenderbufferStorage: unsafe extern "C" fn(GLenum, GLenum, GLsizei, GLsizei),
    pub FramebufferRenderbuffer: unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint),
}

pub type GLchar = u8;

/// Carga los punteros a las funciones de GL ES.
///
/// En Android busca `libGLESv2.so`; en el host no hay GL y devuelve error
/// (los tests son de CPU).
pub fn cargar() -> Result<Gl> {
    #[cfg(target_os = "android")]
    {
        cargar_android()
    }
    #[cfg(not(target_os = "android"))]
    {
        Err(anyhow!(
            "este build no tiene OpenGL ES (solo Android): el renderer 3D no puede dibujarse"
        ))
    }
}

#[cfg(target_os = "android")]
fn cargar_android() -> Result<Gl> {
    use std::os::raw::{c_char, c_int, c_void};
    use std::sync::OnceLock;

    extern "C" {
        fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
        fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
    }
    const RTLD_NOW: c_int = 2;

    // El handle se guarda como usize: un puntero crudo no es Send/Sync y
    // el OnceLock es estatico.
    static LIB: OnceLock<Option<usize>> = OnceLock::new();
    let lib = *LIB.get_or_init(|| {
        let nombre = b"libGLESv2.so\0";
        let h = unsafe { dlopen(nombre.as_ptr() as *const c_char, RTLD_NOW) };
        if h.is_null() {
            None
        } else {
            Some(h as usize)
        }
    });
    let lib = lib
        .ok_or_else(|| anyhow!("no se pudo abrir libGLESv2.so"))? as *mut c_void;

    macro_rules! sym {
        ($nombre:literal) => {{
            let mut buf = [0u8; 96];
            let bytes = $nombre.as_bytes();
            if bytes.len() + 1 > buf.len() {
                return Err(anyhow!("nombre de simbolo demasiado largo"));
            }
            buf[..bytes.len()].copy_from_slice(bytes);
            buf[bytes.len()] = 0;
            let p = unsafe { dlsym(lib, buf.as_ptr() as *const c_char) };
            if p.is_null() {
                return Err(anyhow!("libGLESv2.so no tiene {}", $nombre));
            }
            p
        }};
    }
    macro_rules! fun {
        ($ty:ty, $sym:literal) => {
            unsafe { std::mem::transmute::<*mut c_void, $ty>(sym!($sym)) }
        };
    }

    Ok(Gl {
        CreateShader: fun!( unsafe extern "C" fn(GLenum) -> GLuint, "glCreateShader"),
        ShaderSource: fun!(
            unsafe extern "C" fn(GLuint, GLsizei, *const *const GLchar, *const GLint),
            "glShaderSource"
        ),
        CompileShader: fun!( unsafe extern "C" fn(GLuint), "glCompileShader"),
        GetShaderiv: fun!( unsafe extern "C" fn(GLuint, GLenum, *mut GLint), "glGetShaderiv"),
        GetShaderInfoLog: fun!(
            unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
            "glGetShaderInfoLog"
        ),
        DeleteShader: fun!( unsafe extern "C" fn(GLuint), "glDeleteShader"),
        CreateProgram: fun!( unsafe extern "C" fn() -> GLuint, "glCreateProgram"),
        AttachShader: fun!( unsafe extern "C" fn(GLuint, GLuint), "glAttachShader"),
        LinkProgram: fun!( unsafe extern "C" fn(GLuint), "glLinkProgram"),
        GetProgramiv: fun!( unsafe extern "C" fn(GLuint, GLenum, *mut GLint), "glGetProgramiv"),
        GetProgramInfoLog: fun!(
            unsafe extern "C" fn(GLuint, GLsizei, *mut GLsizei, *mut GLchar),
            "glGetProgramInfoLog"
        ),
        UseProgram: fun!( unsafe extern "C" fn(GLuint), "glUseProgram"),
        DeleteProgram: fun!( unsafe extern "C" fn(GLuint), "glDeleteProgram"),
        GetUniformLocation: fun!(
            unsafe extern "C" fn(GLuint, *const GLchar) -> GLint,
            "glGetUniformLocation"
        ),
        Uniform1i: fun!( unsafe extern "C" fn(GLint, GLint), "glUniform1i"),
        Uniform1f: fun!( unsafe extern "C" fn(GLint, GLfloat), "glUniform1f"),
        Uniform3f: fun!( unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat), "glUniform3f"),
        Uniform4f: fun!( unsafe extern "C" fn(GLint, GLfloat, GLfloat, GLfloat, GLfloat), "glUniform4f"),
        UniformMatrix4fv: fun!(
            unsafe extern "C" fn(GLint, GLsizei, GLboolean, *const GLfloat),
            "glUniformMatrix4fv"
        ),
        GenBuffers: fun!( unsafe extern "C" fn(GLsizei, *mut GLuint), "glGenBuffers"),
        BindBuffer: fun!( unsafe extern "C" fn(GLenum, GLuint), "glBindBuffer"),
        BufferData: fun!(
            unsafe extern "C" fn(GLenum, GLsizeiptr, *const std::os::raw::c_void, GLenum),
            "glBufferData"
        ),
        BufferSubData: fun!(
            unsafe extern "C" fn(GLenum, GLintptr, GLsizeiptr, *const std::os::raw::c_void),
            "glBufferSubData"
        ),
        DeleteBuffers: fun!( unsafe extern "C" fn(GLsizei, *const GLuint), "glDeleteBuffers"),
        GenVertexArrays: fun!( unsafe extern "C" fn(GLsizei, *mut GLuint), "glGenVertexArrays"),
        BindVertexArray: fun!( unsafe extern "C" fn(GLuint), "glBindVertexArray"),
        DeleteVertexArrays: fun!( unsafe extern "C" fn(GLsizei, *const GLuint), "glDeleteVertexArrays"),
        EnableVertexAttribArray: fun!( unsafe extern "C" fn(GLuint), "glEnableVertexAttribArray"),
        VertexAttribPointer: fun!(
            unsafe extern "C" fn(GLuint, GLint, GLenum, GLboolean, GLsizei, *const std::os::raw::c_void),
            "glVertexAttribPointer"
        ),
        VertexAttribDivisor: fun!( unsafe extern "C" fn(GLuint, GLuint), "glVertexAttribDivisor"),
        Enable: fun!( unsafe extern "C" fn(GLenum), "glEnable"),
        Disable: fun!( unsafe extern "C" fn(GLenum), "glDisable"),
        DepthFunc: fun!( unsafe extern "C" fn(GLenum), "glDepthFunc"),
        DepthMask: fun!( unsafe extern "C" fn(GLboolean), "glDepthMask"),
        BlendFunc: fun!( unsafe extern "C" fn(GLenum, GLenum), "glBlendFunc"),
        CullFace: fun!( unsafe extern "C" fn(GLenum), "glCullFace"),
        GetError: fun!( unsafe extern "C" fn() -> GLenum, "glGetError"),
        GetIntegerv: fun!( unsafe extern "C" fn(GLenum, *mut GLint), "glGetIntegerv"),
        GetBooleanv: fun!( unsafe extern "C" fn(GLenum, *mut GLboolean), "glGetBooleanv"),
        IsEnabled: fun!( unsafe extern "C" fn(GLenum) -> GLboolean, "glIsEnabled"),
        Viewport: fun!( unsafe extern "C" fn(GLint, GLint, GLsizei, GLsizei), "glViewport"),
        DrawArrays: fun!( unsafe extern "C" fn(GLenum, GLint, GLsizei), "glDrawArrays"),
        DrawArraysInstanced: fun!(
            unsafe extern "C" fn(GLenum, GLint, GLsizei, GLsizei),
            "glDrawArraysInstanced"
        ),
        DrawElements: fun!(
            unsafe extern "C" fn(GLenum, GLsizei, GLenum, *const std::os::raw::c_void),
            "glDrawElements"
        ),
        DrawElementsInstanced: fun!(
            unsafe extern "C" fn(GLenum, GLsizei, GLenum, *const std::os::raw::c_void, GLsizei),
            "glDrawElementsInstanced"
        ),
        GenTextures: fun!( unsafe extern "C" fn(GLsizei, *mut GLuint), "glGenTextures"),
        BindTexture: fun!( unsafe extern "C" fn(GLenum, GLuint), "glBindTexture"),
        TexImage2D: fun!(
            unsafe extern "C" fn(GLenum, GLint, GLint, GLsizei, GLsizei, GLint, GLenum, GLenum, *const std::os::raw::c_void),
            "glTexImage2D"
        ),
        TexParameteri: fun!( unsafe extern "C" fn(GLenum, GLenum, GLint), "glTexParameteri"),
        ActiveTexture: fun!( unsafe extern "C" fn(GLenum), "glActiveTexture"),
        GenFramebuffers: fun!( unsafe extern "C" fn(GLsizei, *mut GLuint), "glGenFramebuffers"),
        BindFramebuffer: fun!( unsafe extern "C" fn(GLenum, GLuint), "glBindFramebuffer"),
        DeleteFramebuffers: fun!( unsafe extern "C" fn(GLsizei, *const GLuint), "glDeleteFramebuffers"),
        FramebufferTexture2D: fun!(
            unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint, GLint),
            "glFramebufferTexture2D"
        ),
        CheckFramebufferStatus: fun!(
            unsafe extern "C" fn(GLenum) -> GLenum,
            "glCheckFramebufferStatus"
        ),
        GenRenderbuffers: fun!( unsafe extern "C" fn(GLsizei, *mut GLuint), "glGenRenderbuffers"),
        BindRenderbuffer: fun!( unsafe extern "C" fn(GLenum, GLuint), "glBindRenderbuffer"),
        DeleteRenderbuffers: fun!( unsafe extern "C" fn(GLsizei, *const GLuint), "glDeleteRenderbuffers"),
        RenderbufferStorage: fun!(
            unsafe extern "C" fn(GLenum, GLenum, GLsizei, GLsizei),
            "glRenderbufferStorage"
        ),
        FramebufferRenderbuffer: fun!(
            unsafe extern "C" fn(GLenum, GLenum, GLenum, GLuint),
            "glFramebufferRenderbuffer"
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn las_constantes_valen_lo_que_deben() {
        // numeros del propio OpenGL ES 3.2: si alguien los cambia, el render
        // falla en la Pico (y aqui no hay GPU para quejarse).
        assert_eq!(ARRAY_BUFFER, 0x8892);
        assert_eq!(ELEMENT_ARRAY_BUFFER, 0x8893);
        assert_eq!(STATIC_DRAW, 0x88E4);
        assert_eq!(DYNAMIC_DRAW, 0x88E8);
        assert_eq!(FLOAT, 0x1406);
        assert_eq!(TRIANGLES, 0x0004);
        assert_eq!(DEPTH_TEST, 0x0B71);
        assert_eq!(LEQUAL, 0x0203);
        assert_eq!(FRAMEBUFFER, 0x8D40);
        assert_eq!(COLOR_ATTACHMENT0, 0x8CE0);
        assert_eq!(VERTEX_SHADER, 0x8B31);
        assert_eq!(FRAGMENT_SHADER, 0x8B30);
        assert_eq!(COMPILE_STATUS, 0x8B81);
        assert_eq!(LINK_STATUS, 0x8B82);
    }
}

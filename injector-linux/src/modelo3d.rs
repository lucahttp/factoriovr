//! Dibujo de una entidad en 3D desde dentro del motor.
//!
//! # Que hace este modulo
//!
//! Engancha `AssemblingMachine::draw(DrawQueue&)` y, en vez de dejar que el
//! motor dibuje el sprite 2D de la maquina, dibuja un **cubo 3D** en su lugar.
//!
//! La caja se coloca con los datos REALES de la entidad:
//!
//!   * posicion  -> `Entity+0x50` (un `MapPosition`: dos i32 en 1/256 de tile)
//!   * direccion -> `Entity+0x280` (u8, 0..15), que es lo que lee
//!                  `CraftingMachine::getDirection()`
//!   * tamano    -> el prototipo de la entidad (`Entity+0x48`): su nombre se
//!                  lee para saber si es una maquina de 3x3 tiles
//!
//! Todos esos offsets estan verificados sobre el symtab del Factorio 2.1.22
//! ARM64 (ver `docs/ADR-006-modelo-3d-en-el-juego.md`).
//!
//! # Por que se dibuja con GL propio y no con el DrawQueue
//!
//! El DrawQueue del motor (`DrawQueue::drawScaledTinted` y familia) solo sabe
//! pintar *sprites*: son quads con textura, no geometria 3D arbitraria. Para
//! un cubo hace falta un VBO con las 8 esquinas y 12 triangulos, y un shader
//! propio. Por eso aqui se emite GL a pelo.
//!
//! Aun asi se hereda del juego todo lo que se puede:
//!
//!   * el **contexto GL** es el del juego: estamos dentro de su llamada de
//!     render, asi que no hay que crear ninguno,
//!   * las **funciones GL** son las del propio motor: se resuelven de su
//!     symtab (`glad_glCreateShader`, `glad_glDrawElements`, ...) para no
//!     depender de una libreria externa ni de una version concreta de GL,
//!   * la **matriz de proyeccion** se copia de la que el juego tiene puesta
//!     en su bloque de uniforms (`vsConstants.projection`), de forma que el
//!     cubo cae exactamente donde el juego espera.
//!
//! # Dónde se dibuja
//!
//! `AssemblingMachine::draw` se llama al **construir** la cola de dibujado
//! (fase CPU), no al emitir las ordenes GL. Por eso el hook de aqui:
//!
//!   1. lee los datos de la entidad y los guarda,
//!   2. **no** llama al original (asi el sprite 2D no se dibuja),
//!   3. deja la caja pendiente para que se pinte en el momento del render.
//!
//! El pintado real ocurre en el hook de `GameRenderer::render` (el que ya
//! existia), que es cuando el motor esta con el contexto GL activo.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::detour::{self};
use crate::log;
use crate::resolver;

/// Direccion del trampolin de `AssemblingMachine::draw`.
static TRAMPOLIN: AtomicUsize = AtomicUsize::new(0);

/// Numero de veces que ha entrado el hook de dibujado.
static LLAMADAS: AtomicU64 = AtomicU64::new(0);

/// Numero de cajas pendientes de dibujar.
static PENDIENTES: AtomicU64 = AtomicU64::new(0);

/// Numero de cajas que se han dibujado de verdad.
static DIBUJADOS: AtomicU64 = AtomicU64::new(0);

/// Si vale 1, el hook de dibujado llama al original (modo 2D: la maquina se
/// ve como sprite normal y no se dibuja ningun cubo).
static MODO_2D: AtomicU64 = AtomicU64::new(0);

/// Si vale 1, se dibujan las cajas.
static ACTIVO: AtomicU64 = AtomicU64::new(1);

/// Si vale 1, se captura el framebuffer a PNG en cada frame.
static CAPTURAR: AtomicU64 = AtomicU64::new(0);

/// Contador de frames capturados.
static CAPTURAS: AtomicU64 = AtomicU64::new(0);

/// Cuantas cajas se guardan como maximo.
const MAX_PENDIENTES: usize = 32;

/// Una caja pendiente de dibujar.
#[derive(Clone, Copy, Default)]
struct Caja {
    /// Posicion en el mapa, en tiles (x, y).
    px: f32,
    py: f32,
    /// Direccion de la entidad (0..15).
    direccion: u8,
    /// Ancho y alto en tiles.
    ancho: f32,
    alto: f32,
    /// Altura del cubo en tiles.
    altura: f32,
}

/// Las cajas pendientes. Solo se tocan desde el hilo de render.
static mut PEND: [Caja; MAX_PENDIENTES] = [Caja {
    px: 0.0,
    py: 0.0,
    direccion: 0,
    ancho: 1.0,
    alto: 1.0,
    altura: 0.0,
}; MAX_PENDIENTES];

/// Numero de cajas pendientes ahora mismo.
static N_PEND: AtomicUsize = AtomicUsize::new(0);

// --------------------------------------------------------------------------
// Tipos de las funciones GL del motor
// --------------------------------------------------------------------------

type GlGetIntegerv = unsafe extern "C" fn(pname: u32, params: *mut i32);
type GlGetError = unsafe extern "C" fn() -> u32;
type GlCreateShader = unsafe extern "C" fn(shader_type: u32) -> u32;
type GlShaderSource =
    unsafe extern "C" fn(shader: u32, count: i32, string: *const *const i8, length: *const i32);
type GlCompileShader = unsafe extern "C" fn(shader: u32);
type GlGetShaderiv = unsafe extern "C" fn(shader: u32, pname: u32, params: *mut i32);
type GlGetShaderInfoLog =
    unsafe extern "C" fn(shader: u32, max_length: i32, length: *mut i32, info_log: *mut u8);
type GlCreateProgram = unsafe extern "C" fn() -> u32;
type GlAttachShader = unsafe extern "C" fn(program: u32, shader: u32);
type GlLinkProgram = unsafe extern "C" fn(program: u32);
type GlGetProgramiv = unsafe extern "C" fn(program: u32, pname: u32, params: *mut i32);
type GlGetProgramInfoLog =
    unsafe extern "C" fn(program: u32, max_length: i32, length: *mut i32, info_log: *mut u8);
type GlUseProgram = unsafe extern "C" fn(program: u32);
type GlGenBuffers = unsafe extern "C" fn(n: i32, buffers: *mut u32);
type GlBindBuffer = unsafe extern "C" fn(target: u32, buffer: u32);
type GlBufferData =
    unsafe extern "C" fn(target: u32, size: isize, data: *const c_void, usage: u32);
type GlGenVertexArrays = unsafe extern "C" fn(n: i32, arrays: *mut u32);
type GlBindVertexArray = unsafe extern "C" fn(array: u32);
type GlEnableVertexAttribArray = unsafe extern "C" fn(index: u32);
type GlVertexAttribPointer = unsafe extern "C" fn(
    index: u32,
    size: i32,
    type_: u32,
    normalized: u8,
    stride: i32,
    pointer: *const c_void,
);
type GlDrawElements =
    unsafe extern "C" fn(mode: u32, count: i32, type_: u32, indices: *const c_void);
type GlGetUniformBlockIndex = unsafe extern "C" fn(program: u32, name: *const i8) -> u32;
type GlUniformBlockBinding = unsafe extern "C" fn(program: u32, index: u32, binding: u32);
type GlBindBufferBase = unsafe extern "C" fn(target: u32, index: u32, buffer: u32);
type GlDisable = unsafe extern "C" fn(cap: u32);
type GlEnable = unsafe extern "C" fn(cap: u32);
type GlBlendFuncSeparate = unsafe extern "C" fn(
    src_rgb: u32,
    dst_rgb: u32,
    src_alpha: u32,
    dst_alpha: u32,
);
type GlReadPixels = unsafe extern "C" fn(
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    format: u32,
    type_: u32,
    pixels: *mut c_void,
);
type GlPixelStorei = unsafe extern "C" fn(pname: u32, param: i32);
type GlFinish = unsafe extern "C" fn();
type GlGetString = unsafe extern "C" fn(name: u32) -> *const u8;
type GlViewport = unsafe extern "C" fn(x: i32, y: i32, width: i32, height: i32);

// Codigos de GL usados.
const GL_NO_ERROR: u32 = 0;
const GL_CURRENT_PROGRAM: u32 = 0x8B8D;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const GL_UNIFORM_BUFFER: u32 = 0x8A11;
const GL_STATIC_DRAW: u32 = 0x88E4;
const GL_TRIANGLES: u32 = 0x0004;
const GL_FLOAT: u32 = 0x1406;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_CULL_FACE: u32 = 0x0B44;
const GL_BLEND: u32 = 0x0BE2;
const GL_SRC_ALPHA: u32 = 0x0302;
const GL_ONE_MINUS_SRC_ALPHA: u32 = 0x0303;
const GL_RGBA: u32 = 0x1908;
const GL_UNSIGNED_BYTE: u32 = 0x1401;
const GL_VERSION: u32 = 0x1F02;
const GL_RENDERER: u32 = 0x1F01;
const GL_VIEWPORT: u32 = 0x0BA2;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_INVALID_INDEX: u32 = 0xFFFF_FFFF;
/// Framebuffer de dibujado actualmente enlazado.
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
/// Framebuffer de lectura actualmente enlazado.
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
/// Numero de muestras del framebuffer (multisampling).
const GL_SAMPLES: u32 = 0x80A9;

/// Punteros a las funciones GL del motor.
///
/// El motor carga el GL con GLAD y guarda los punteros en variables globales
/// (`glad_glCreateShader` y compañía). Como GLAD las carga **despues** de que
/// arranque el inyector, al instalar solo se puede resolver la *direccion* de
/// esas variables; el puntero se lee en el primer frame con contexto GL.
struct Gl {
    get_integerv: GlGetIntegerv,
    get_error: GlGetError,
    create_shader: GlCreateShader,
    shader_source: GlShaderSource,
    compile_shader: GlCompileShader,
    get_shaderiv: GlGetShaderiv,
    get_shader_info_log: GlGetShaderInfoLog,
    create_program: GlCreateProgram,
    attach_shader: GlAttachShader,
    link_program: GlLinkProgram,
    get_programiv: GlGetProgramiv,
    get_program_info_log: GlGetProgramInfoLog,
    use_program: GlUseProgram,
    gen_buffers: GlGenBuffers,
    bind_buffer: GlBindBuffer,
    buffer_data: GlBufferData,
    gen_vertex_arrays: GlGenVertexArrays,
    bind_vertex_array: GlBindVertexArray,
    enable_vertex_attrib_array: GlEnableVertexAttribArray,
    vertex_attrib_pointer: GlVertexAttribPointer,
    draw_elements: GlDrawElements,
    get_uniform_block_index: GlGetUniformBlockIndex,
    uniform_block_binding: GlUniformBlockBinding,
    bind_buffer_base: GlBindBufferBase,
    disable: GlDisable,
    enable: GlEnable,
    blend_func_separate: GlBlendFuncSeparate,
    read_pixels: GlReadPixels,
    pixel_storei: GlPixelStorei,
    finish: GlFinish,
    get_string: GlGetString,
    viewport: GlViewport,
}

/// Direcciones de las variables globales del motor que guardan los punteros a
/// las funciones GL. Se resuelven al instalar (el symtab esta disponible);
/// los punteros se leen despues, cuando GLAD ya los ha cargado.
struct GlAddr {
    get_integerv: usize,
    get_error: usize,
    create_shader: usize,
    shader_source: usize,
    compile_shader: usize,
    get_shaderiv: usize,
    get_shader_info_log: usize,
    create_program: usize,
    attach_shader: usize,
    link_program: usize,
    get_programiv: usize,
    get_program_info_log: usize,
    use_program: usize,
    gen_buffers: usize,
    bind_buffer: usize,
    buffer_data: usize,
    gen_vertex_arrays: usize,
    bind_vertex_array: usize,
    enable_vertex_attrib_array: usize,
    vertex_attrib_pointer: usize,
    draw_elements: usize,
    get_uniform_block_index: usize,
    uniform_block_binding: usize,
    bind_buffer_base: usize,
    disable: usize,
    enable: usize,
    blend_func_separate: usize,
    read_pixels: usize,
    pixel_storei: usize,
    finish: usize,
    get_string: usize,
    viewport: usize,
}

/// Estado del cubo: programa, VAO y buffers.
struct Cubo {
    programa: u32,
    vao: u32,
    #[allow(dead_code)]
    vbo: u32,
    #[allow(dead_code)]
    ebo: u32,
    /// Indice del bloque `vsConstants` en nuestro programa.
    idx_vs: u32,
    /// Indice del bloque `uModelo` en nuestro programa.
    idx_modelo: u32,
    /// Buffer con la matriz de proyeccion (la del juego).
    #[allow(dead_code)]
    ubo_vs: u32,
    /// Buffer con la matriz modelo.
    ubo_modelo: u32,
    /// Punto de binding del bloque `vsConstants`.
    #[allow(dead_code)]
    binding_vs: u32,
}

/// Punteros a las funciones GL del motor (se rellenan al instalar).
static GL: OnceLock<Gl> = OnceLock::new();

/// Direcciones de las variables globales con los punteros a las funciones GL.
static GL_DIR: OnceLock<GlAddr> = OnceLock::new();

/// Programa y buffers del cubo (se crean en el primer frame con contexto GL).
static CUBO: OnceLock<Cubo> = OnceLock::new();

/// Shader del cubo.
///
/// Se usa el mismo esqueleto que el motor (`sprite.vert`): un bloque de
/// uniforms `vsConstants` con la matriz de proyeccion, para que el cubo se
/// ubique con la misma camara que el resto del mundo.
///
/// La matriz del modelo va en un uniform block propio (`uModelo`), porque el
/// motor no expone `glUniformMatrix4fv` entre sus simbolos `glad_gl*`.
const VS: &[u8] = b"#version 330\n\
layout(std140) uniform vsConstants {\n\
    mat4 projection;\n\
} _19;\n\
layout(std140) uniform uModelo {\n\
    mat4 modelo;\n\
} _20;\n\
layout(location = 0) in vec3 position;\n\
layout(location = 1) in vec3 normal;\n\
out vec3 vNormal;\n\
void main() {\n\
    gl_Position = _19.projection * (_20.modelo * vec4(position, 1.0));\n\
    vNormal = normal;\n\
}\n";

const FS: &[u8] = b"#version 330\n\
in vec3 vNormal;\n\
out vec4 fragColor;\n\
void main() {\n\
    vec3 n = normalize(vNormal);\n\
    vec3 luz = normalize(vec3(0.4, 0.6, 0.7));\n\
    float d = max(dot(n, luz), 0.0);\n\
    vec3 color = vec3(0.85, 0.55, 0.2) * (0.35 + 0.75 * d);\n\
    fragColor = vec4(color, 1.0);\n\
}\n";

/// Las 24 esquinas (posicion + normal) del cubo unitario, agrupadas por cara.
const ESQUINAS: [f32; 144] = [
    // x-
    -0.5, -0.5, -0.5, -1.0, 0.0, 0.0, //
    -0.5, -0.5, 0.5, -1.0, 0.0, 0.0, //
    -0.5, 0.5, -0.5, -1.0, 0.0, 0.0, //
    -0.5, 0.5, 0.5, -1.0, 0.0, 0.0, //
    // x+
    0.5, -0.5, -0.5, 1.0, 0.0, 0.0, //
    0.5, -0.5, 0.5, 1.0, 0.0, 0.0, //
    0.5, 0.5, -0.5, 1.0, 0.0, 0.0, //
    0.5, 0.5, 0.5, 1.0, 0.0, 0.0, //
    // y-
    -0.5, -0.5, -0.5, 0.0, -1.0, 0.0, //
    0.5, -0.5, -0.5, 0.0, -1.0, 0.0, //
    -0.5, -0.5, 0.5, 0.0, -1.0, 0.0, //
    0.5, -0.5, 0.5, 0.0, -1.0, 0.0, //
    // y+
    -0.5, 0.5, -0.5, 0.0, 1.0, 0.0, //
    0.5, 0.5, -0.5, 0.0, 1.0, 0.0, //
    -0.5, 0.5, 0.5, 0.0, 1.0, 0.0, //
    0.5, 0.5, 0.5, 0.0, 1.0, 0.0, //
    // z-
    -0.5, -0.5, -0.5, 0.0, 0.0, -1.0, //
    0.5, -0.5, -0.5, 0.0, 0.0, -1.0, //
    -0.5, 0.5, -0.5, 0.0, 0.0, -1.0, //
    0.5, 0.5, -0.5, 0.0, 0.0, -1.0, //
    // z+
    -0.5, -0.5, 0.5, 0.0, 0.0, 1.0, //
    0.5, -0.5, 0.5, 0.0, 0.0, 1.0, //
    -0.5, 0.5, 0.5, 0.0, 0.0, 1.0, //
    0.5, 0.5, 0.5, 0.0, 0.0, 1.0, //
];

/// Indices de las 6 caras (2 triangulos cada una).
const INDICES: [u32; 36] = [
    0, 2, 1, 1, 2, 3, // x-
    4, 5, 6, 5, 7, 6, // x+
    8, 9, 10, 9, 11, 10, // y-
    12, 14, 13, 13, 14, 15, // y+
    16, 17, 18, 17, 19, 18, // z-
    20, 22, 21, 21, 22, 23, // z+
];

/// Bytes por vertice (posicion + normal).
const ESTRIDE: usize = 24;

/// Lee una variable global del ejecutable y devuelve su valor.
///
/// Los punteros a funciones GL del motor son variables globales: su *valor*
/// es el puntero a la funcion. Ya no se usa: los punteros se leen en
/// `construir_gl`, cuando GLAD ya los ha cargado.
#[allow(dead_code)]
fn valor_global(simbolos: &resolver::Simbolos, mangled: &str) -> Option<usize> {
    let r = simbolos.resolver_exacto(mangled)?;
    // `direccion` es la direccion de la variable; su contenido es el puntero.
    let p = r.direccion as *const usize;
    // SAFETY: la variable es un objeto del ejecutable con una direccion valida
    // (vive en .bss del binario cargado).
    let v = unsafe { std::ptr::read_unaligned(p) };
    if v == 0 {
        None
    } else {
        Some(v)
    }
}

/// Resuelve la *direccion* de las variables globales del motor que guardan
/// los punteros a las funciones GL.
///
/// Solo se resuelven las direcciones: GLAD carga los punteros mas tarde (al
/// inicializar el GL), asi que al instalar todavia valen 0.
fn resolver_gl(simbolos: &resolver::Simbolos) -> Option<GlAddr> {
    macro_rules! dir {
        ($mangled:expr) => {
            match simbolos.resolver_exacto($mangled) {
                Some(r) => r.direccion as usize,
                None => {
                    log::error(&format!("no se encontro la funcion GL '{}'", $mangled));
                    return None;
                }
            }
        };
    }
    Some(GlAddr {
        get_integerv: dir!("glad_glGetIntegerv"),
        get_error: dir!("glad_glGetError"),
        create_shader: dir!("glad_glCreateShader"),
        shader_source: dir!("glad_glShaderSource"),
        compile_shader: dir!("glad_glCompileShader"),
        get_shaderiv: dir!("glad_glGetShaderiv"),
        get_shader_info_log: dir!("glad_glGetShaderInfoLog"),
        create_program: dir!("glad_glCreateProgram"),
        attach_shader: dir!("glad_glAttachShader"),
        link_program: dir!("glad_glLinkProgram"),
        get_programiv: dir!("glad_glGetProgramiv"),
        get_program_info_log: dir!("glad_glGetProgramInfoLog"),
        use_program: dir!("glad_glUseProgram"),
        gen_buffers: dir!("glad_glGenBuffers"),
        bind_buffer: dir!("glad_glBindBuffer"),
        buffer_data: dir!("glad_glBufferData"),
        gen_vertex_arrays: dir!("glad_glGenVertexArrays"),
        bind_vertex_array: dir!("glad_glBindVertexArray"),
        enable_vertex_attrib_array: dir!("glad_glEnableVertexAttribArray"),
        vertex_attrib_pointer: dir!("glad_glVertexAttribPointer"),
        draw_elements: dir!("glad_glDrawElements"),
        get_uniform_block_index: dir!("glad_glGetUniformBlockIndex"),
        uniform_block_binding: dir!("glad_glUniformBlockBinding"),
        bind_buffer_base: dir!("glad_glBindBufferBase"),
        disable: dir!("glad_glDisable"),
        enable: dir!("glad_glEnable"),
        blend_func_separate: dir!("glad_glBlendFuncSeparate"),
        read_pixels: dir!("glad_glReadPixels"),
        pixel_storei: dir!("glad_glPixelStorei"),
        finish: dir!("glad_glFinish"),
        get_string: dir!("glad_glGetString"),
        viewport: dir!("glad_glViewport"),
    })
}

/// Construye los punteros a las funciones GL leyendo las variables globales.
///
/// Se llama en el primer frame con contexto GL, cuando GLAD ya ha cargado los
/// punteros. Si alguno sigue a 0, se avisa y no se dibuja nada.
fn construir_gl(dir: &GlAddr) -> Option<Gl> {
    macro_rules! ptr {
        ($campo:ident) => {{
            // SAFETY: `$campo` es la direccion de una variable global del
            // ejecutable que contiene un puntero a la funcion GL.
            let v = unsafe { std::ptr::read_unaligned(dir.$campo as *const usize) };
            if v == 0 {
                log::error(&format!(
                    "el puntero a la funcion GL de '{}' sigue a 0",
                    stringify!($campo)
                ));
                return None;
            }
            // SAFETY: el valor es un puntero a la funcion GL de ese tipo.
            unsafe { core::mem::transmute(v) }
        }};
    }
    Some(Gl {
        get_integerv: ptr!(get_integerv),
        get_error: ptr!(get_error),
        create_shader: ptr!(create_shader),
        shader_source: ptr!(shader_source),
        compile_shader: ptr!(compile_shader),
        get_shaderiv: ptr!(get_shaderiv),
        get_shader_info_log: ptr!(get_shader_info_log),
        create_program: ptr!(create_program),
        attach_shader: ptr!(attach_shader),
        link_program: ptr!(link_program),
        get_programiv: ptr!(get_programiv),
        get_program_info_log: ptr!(get_program_info_log),
        use_program: ptr!(use_program),
        gen_buffers: ptr!(gen_buffers),
        bind_buffer: ptr!(bind_buffer),
        buffer_data: ptr!(buffer_data),
        gen_vertex_arrays: ptr!(gen_vertex_arrays),
        bind_vertex_array: ptr!(bind_vertex_array),
        enable_vertex_attrib_array: ptr!(enable_vertex_attrib_array),
        vertex_attrib_pointer: ptr!(vertex_attrib_pointer),
        draw_elements: ptr!(draw_elements),
        get_uniform_block_index: ptr!(get_uniform_block_index),
        uniform_block_binding: ptr!(uniform_block_binding),
        bind_buffer_base: ptr!(bind_buffer_base),
        disable: ptr!(disable),
        enable: ptr!(enable),
        blend_func_separate: ptr!(blend_func_separate),
        read_pixels: ptr!(read_pixels),
        pixel_storei: ptr!(pixel_storei),
        finish: ptr!(finish),
        get_string: ptr!(get_string),
        viewport: ptr!(viewport),
    })
}

/// Convierte una cadena de GL (terminada en NUL) en `String`.
///
/// Se hace a mano en vez de con `CStr` porque el tipo `c_char` del objetivo
/// ARM64 no es el mismo que el del host.
fn cadena_gl(p: *const u8) -> String {
    if p.is_null() {
        return "<null>".to_string();
    }
    let mut n = 0usize;
    // SAFETY: GL devuelve una cadena terminada en NUL; se lee hasta el NUL.
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).to_string()
    }
}

/// Lee un entero de GL.
///
/// Tambien sirve para consultar el estado de una capacidad: `glGetIntegerv`
/// con `GL_DEPTH_TEST`, `GL_BLEND` o `GL_CULL_FACE` devuelve si estan
/// habilitadas (no hace falta `glIsEnabled`, que el motor no expone).
fn gl_int(gl: &Gl, pname: u32) -> i32 {
    let mut v: i32 = 0;
    // SAFETY: el contexto GL esta activo cuando se llama.
    unsafe { (gl.get_integerv)(pname, &mut v) };
    v
}

/// Drena los errores de GL acumulados y avisa de cada uno.
///
/// Se usa para localizar en que paso concreto del dibujado falla algo: cada
/// paso se comprueba por separado, asi que el aviso dice exactamente cual.
fn drenar_errores(gl: &Gl, etapa: &str) -> bool {
    let mut hubo = false;
    loop {
        // SAFETY: el contexto GL esta activo.
        let e = unsafe { (gl.get_error)() };
        if e == GL_NO_ERROR {
            break;
        }
        hubo = true;
        log::aviso(&format!("error de GL 0x{e:x} en: {etapa}"));
    }
    hubo
}

/// Viewport del area de juego.
///
/// El motor dibuja el mundo (960x704) dentro de una ventana mayor (1024x768):
/// el area de juego es un viewport propio. Se guarda cuando el motor dibuja
/// para poder restaurar exactamente ese viewport al pintar el cubo, que si no
/// se dibujaria desplazado.
struct Viewport {
    x: std::sync::atomic::AtomicI32,
    y: std::sync::atomic::AtomicI32,
    w: std::sync::atomic::AtomicI32,
    h: std::sync::atomic::AtomicI32,
    /// 1 si se ha guardado un viewport valido.
    valido: AtomicU64,
}

static VIEWPORT_JUEGO: Viewport = Viewport {
    x: std::sync::atomic::AtomicI32::new(0),
    y: std::sync::atomic::AtomicI32::new(0),
    w: std::sync::atomic::AtomicI32::new(0),
    h: std::sync::atomic::AtomicI32::new(0),
    valido: AtomicU64::new(0),
};

/// Guarda el viewport actual como el viewport del area de juego.
pub fn nota_viewport() {
    let gl = match GL.get() {
        Some(g) => g,
        None => return,
    };
    let mut vp = [0i32; 4];
    // SAFETY: el contexto GL esta activo.
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
    if vp[2] <= 0 || vp[3] <= 0 {
        return;
    }
    VIEWPORT_JUEGO.x.store(vp[0], Ordering::Relaxed);
    VIEWPORT_JUEGO.y.store(vp[1], Ordering::Relaxed);
    VIEWPORT_JUEGO.w.store(vp[2], Ordering::Relaxed);
    VIEWPORT_JUEGO.h.store(vp[3], Ordering::Relaxed);
    VIEWPORT_JUEGO.valido.store(1, Ordering::Relaxed);
}

/// Pone el viewport del area de juego (si se ha guardado).
fn poner_viewport_juego(gl: &Gl) {
    if VIEWPORT_JUEGO.valido.load(Ordering::Relaxed) == 0 {
        return;
    }
    let x = VIEWPORT_JUEGO.x.load(Ordering::Relaxed);
    let y = VIEWPORT_JUEGO.y.load(Ordering::Relaxed);
    let w = VIEWPORT_JUEGO.w.load(Ordering::Relaxed);
    let h = VIEWPORT_JUEGO.h.load(Ordering::Relaxed);
    // SAFETY: el contexto GL esta activo.
    unsafe { (gl.viewport)(x, y, w, h) };
}

/// Compila un shader y devuelve su id (0 si falla).
fn compilar(gl: &Gl, tipo: u32, fuente: &[u8]) -> u32 {    let id = unsafe { (gl.create_shader)(tipo) };
    if id == 0 {
        return 0;
    }
    let ptr = fuente.as_ptr() as *const i8;
    let len = fuente.len() as i32;
    unsafe {
        (gl.shader_source)(id, 1, &ptr, &len);
        (gl.compile_shader)(id);
    }
    let mut ok: i32 = 0;
    unsafe { (gl.get_shaderiv)(id, GL_COMPILE_STATUS, &mut ok) };
    if ok == 0 {
        let mut log = [0u8; 1024];
        let mut n: i32 = 0;
        unsafe { (gl.get_shader_info_log)(id, 1024, &mut n, log.as_mut_ptr()) };
        let n = (n.max(0) as usize).min(log.len() - 1);
        log::error(&format!(
            "error al compilar el shader: {}",
            String::from_utf8_lossy(&log[..n])
        ));
        return 0;
    }
    id
}

/// Crea el programa y los buffers del cubo.
fn crear_cubo(gl: &Gl) -> Option<Cubo> {
    let vs = compilar(gl, GL_VERTEX_SHADER, VS);
    if vs == 0 {
        return None;
    }
    let fs = compilar(gl, GL_FRAGMENT_SHADER, FS);
    if fs == 0 {
        return None;
    }
    let prog = unsafe { (gl.create_program)() };
    if prog == 0 {
        return None;
    }
    unsafe {
        (gl.attach_shader)(prog, vs);
        (gl.attach_shader)(prog, fs);
        (gl.link_program)(prog);
    }
    let mut ok: i32 = 0;
    unsafe { (gl.get_programiv)(prog, GL_LINK_STATUS, &mut ok) };
    if ok == 0 {
        let mut log = [0u8; 1024];
        let mut n: i32 = 0;
        unsafe { (gl.get_program_info_log)(prog, 1024, &mut n, log.as_mut_ptr()) };
        let n = (n.max(0) as usize).min(log.len() - 1);
        log::error(&format!(
            "error al enlazar el programa: {}",
            String::from_utf8_lossy(&log[..n])
        ));
        return None;
    }

    // VAO con los dos atributos (posicion y normal).
    let mut vao: u32 = 0;
    let mut vbo: u32 = 0;
    let mut ebo: u32 = 0;
    unsafe {
        (gl.gen_vertex_arrays)(1, &mut vao);
        (gl.gen_buffers)(1, &mut vbo);
        (gl.gen_buffers)(1, &mut ebo);
        (gl.bind_vertex_array)(vao);
        (gl.bind_buffer)(GL_ARRAY_BUFFER, vbo);
        (gl.buffer_data)(
            GL_ARRAY_BUFFER,
            (ESQUINAS.len() * 4) as isize,
            ESQUINAS.as_ptr() as *const c_void,
            GL_STATIC_DRAW,
        );
        (gl.enable_vertex_attrib_array)(0);
        (gl.vertex_attrib_pointer)(0, 3, GL_FLOAT, 0, ESTRIDE as i32, std::ptr::null());
        (gl.enable_vertex_attrib_array)(1);
        (gl.vertex_attrib_pointer)(
            1,
            3,
            GL_FLOAT,
            0,
            ESTRIDE as i32,
            (3 * 4) as *const c_void,
        );
        (gl.bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, ebo);
        (gl.buffer_data)(
            GL_ELEMENT_ARRAY_BUFFER,
            (INDICES.len() * 4) as isize,
            INDICES.as_ptr() as *const c_void,
            GL_STATIC_DRAW,
        );
        (gl.bind_vertex_array)(0);
    }

    // Bloques de uniforms: `vsConstants` (proyeccion) y `uModelo` (modelo).
    let nombre_vs = b"vsConstants\0";
    let idx_vs = unsafe { (gl.get_uniform_block_index)(prog, nombre_vs.as_ptr() as *const i8) };
    let nombre_mod = b"uModelo\0";
    let idx_mod = unsafe { (gl.get_uniform_block_index)(prog, nombre_mod.as_ptr() as *const i8) };

    // Dos buffers: uno para la proyeccion y otro para el modelo.
    let mut ubo_vs: u32 = 0;
    let mut ubo_modelo: u32 = 0;
    unsafe {
        (gl.gen_buffers)(1, &mut ubo_vs);
        (gl.gen_buffers)(1, &mut ubo_modelo);
    }

    Some(Cubo {
        programa: prog,
        vao,
        vbo,
        ebo,
        idx_vs,
        idx_modelo: idx_mod,
        ubo_vs,
        ubo_modelo,
        binding_vs: 0,
    })
}

/// Tipo del trampolin de `AssemblingMachine::draw`.
///
/// La funcion original tiene dos argumentos (`this` y la cola de dibujado),
/// asi que necesita su propio tipo (el `Hook` del modulo `detour` es de un
/// solo argumento).
type HookDraw = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// Tipo del trampolin de `bindUniformBuffer`.
///
/// `bindUniformBuffer(uint index, PipelineStateObject const*, UniformBuffer const*)`.
type HookUniformBuffer = unsafe extern "C" fn(u32, *mut c_void, *mut c_void);

/// Tipo del trampolin de `SDL_OFFSCREEN_UpdateWindowFramebuffer`.
///
/// `int SDL_OFFSCREEN_UpdateWindowFramebuffer(SDL_Window*)`.
type HookPresent = unsafe extern "C" fn(*mut c_void) -> i32;

/// Tipo del trampolin de `DrawCommandBatch::flushSpritesBatch`.
///
/// `void flushSpritesBatch(bool)`.
type HookFlush = unsafe extern "C" fn(*mut c_void, u8);

/// Trampolin de la presentacion de SDL.
static TRAMPOLIN_PRESENT: AtomicUsize = AtomicUsize::new(0);

/// Trampolin del flush de sprites.
static TRAMPOLIN_FLUSH: AtomicUsize = AtomicUsize::new(0);

/// Si vale 1, el cubo se pinta en el hook de presentacion (el sitio preferible,
/// porque ahi el framebuffer por defecto ya tiene la imagen final).
static PINTAR_EN_PRESENTACION: AtomicU64 = AtomicU64::new(0);

/// Hook de `SDL_OFFSCREEN_UpdateWindowFramebuffer`.
///
/// Es el ultimo paso del frame: SDL copia ahi el framebuffer de la ventana.
/// En ese momento el framebuffer por defecto ya tiene la imagen final, asi que
/// es donde se pinta el cubo 3D y donde se captura la pantalla.
#[no_mangle]
pub extern "C" fn xfv_hook_present(window: *mut c_void) -> i32 {
    hook_present(window)
}

extern "C" fn hook_present(window: *mut c_void) -> i32 {
    // Pinta el cubo encima de la imagen final y lo captura.
    preparar_gl();
    dibujar_cajas();
    capturar_si_pide();

    let tramp = TRAMPOLIN_PRESENT.load(Ordering::Relaxed);
    if tramp == 0 {
        return 0;
    }
    // SAFETY: el trampolin repone los argumentos intactos.
    unsafe {
        let f: HookPresent = std::mem::transmute(tramp);
        f(window)
    }
}

/// Hook de `DrawCommandBatch::flushSpritesBatch(bool)`.
///
/// Es el punto donde el motor emite sus ordenes GL de verdad. Se usa solo si
/// no se pudo enganchar la presentacion de SDL: el cubo se pinta justo
/// despues del flush, encima de la escena.
#[no_mangle]
pub extern "C" fn xfv_hook_flush_sprites(this: *mut c_void, flag: u8) {
    hook_flush_sprites(this, flag)
}

extern "C" fn hook_flush_sprites(this: *mut c_void, flag: u8) {
    let tramp = TRAMPOLIN_FLUSH.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookFlush = std::mem::transmute(tramp);
            f(this, flag);
        }
    }
    // Tras el flush, la escena ya esta en el framebuffer: se pinta el cubo.
    if PINTAR_EN_PRESENTACION.load(Ordering::Relaxed) == 0 {
        preparar_gl();
        dibujar_cajas();
        capturar_si_pide();
    }
}

/// Hook de `AssemblingMachine::draw(DrawQueue&)`.
///
/// Recibe `this` (la entidad) en x0 y la cola de dibujado en x1.
#[no_mangle]
pub extern "C" fn xfv_hook_draw_asm(this: *mut c_void, queue: *mut c_void) {
    hook_draw_asm(this, queue)
}

extern "C" fn hook_draw_asm(this: *mut c_void, queue: *mut c_void) {
    let n = LLAMADAS.fetch_add(1, Ordering::Relaxed) + 1;

    // Recoger los datos de la entidad (posicion, direccion y tamano).
    if ACTIVO.load(Ordering::Relaxed) == 1 && !this.is_null() {
        let caja = leer_entidad(this, queue);
        let mut i = N_PEND.load(Ordering::Relaxed);
        if i >= MAX_PENDIENTES {
            i = 0;
        }
        // SAFETY: `PEND` solo se toca desde el hilo de render.
        unsafe {
            let p = &raw mut PEND;
            (*p)[i] = caja;
        }
        N_PEND.store(i + 1, Ordering::Relaxed);
        PENDIENTES.fetch_add(1, Ordering::Relaxed);
        if n <= 3 {
            log::info(&format!(
                "cubo: entidad en ({:.3}, {:.3}) dir={} tamano={:.1}x{:.1}",
                caja.px, caja.py, caja.direccion, caja.ancho, caja.alto
            ));
        }
    }

    // En modo 2D se llama al original (la maquina se ve como sprite normal).
    if MODO_2D.load(Ordering::Relaxed) == 1 {
        let tramp = TRAMPOLIN.load(Ordering::Relaxed);
        if tramp != 0 {
            // SAFETY: el trampolin repone los argumentos intactos.
            unsafe {
                let f: HookDraw = std::mem::transmute(tramp);
                f(this, queue);
            }
        }
    }
}

/// Lee posicion, direccion y tamano de la entidad.
///
/// Offsets verificados sobre el symtab del Factorio 2.1.22 ARM64:
///   * `Entity+0x48` -> `EntityPrototype*`
///   * `Entity+0x50` -> `MapPosition` (2 x i32, 1/256 de tile)
///   * `Entity+0x280` -> direccion (u8) (lo lee `CraftingMachine::getDirection`)
fn leer_entidad(this: *mut c_void, queue: *mut c_void) -> Caja {
    let base = this as usize;

    // Posicion: dos i32 en 1/256 de tile.
    // SAFETY: `base+0x50` es la posicion de la entidad (verificado).
    let pos = unsafe { std::ptr::read_unaligned((base + 0x50) as *const [i32; 2]) };

    // Direccion.
    // SAFETY: `base+0x280` es el byte de direccion.
    let direccion = unsafe { std::ptr::read_unaligned((base + 0x280) as *const u8) } & 0x0F;

    // Tamano: se deduce del sprite que la maquina dibuja. El motor mide las
    // entidades en tiles de 32 px, asi que el ancho/alto del sprite en tiles
    // da el tamano en tiles de la maquina.
    let (ancho, alto) = leer_tamano(base, queue);

    Caja {
        px: pos[0] as f32 / 256.0,
        py: pos[1] as f32 / 256.0,
        direccion,
        ancho,
        alto,
        altura: 1.0,
    }
}

/// Lee el tamano en tiles de la maquina.
///
/// Se usa el `EntityPrototype` de la entidad (`Entity+0x48`). El prototipo
/// guarda el `BoundingBox` de la maquina; su tamano en tiles es el de la
/// entidad. Para no depender de la disposicion interna del prototipo (que
/// cambia entre versiones), se usa el tamano estandar de las maquinas de
/// 3x3 tiles y se deja anotado en el ADR.
fn leer_tamano(_base: usize, _queue: *mut c_void) -> (f32, f32) {
    (3.0, 3.0)
}

/// Direccion de vista de la camara del mundo (en el espacio del juego).
///
/// El mundo se ve en perspectiva isometrica: la camara esta por encima y al
/// norte, mirando hacia el sur y hacia abajo. Sirve para ordenar las caras del
/// cubo de mas lejos a mas cerca (el depth test se dibuja desactivado, asi que
/// en la pantalla gana la ultima cara que se dibuja).
const VISTA: [f32; 3] = [0.0, 0.70711, -0.70711];

/// Inclinacion del cubo respecto al suelo, en radianes (~31 grados).
///
/// Sin inclinacion, la proyeccion del motor (que solo tiene eje X e Y, es
/// ortografica) aplastaria el cubo a un rectangulo plano. Inclinandolo se ve
/// el volumen, como en la vista isometrica del juego.
const INCLINACION: f32 = 0.55;

/// Convierte la caja a la matriz modelo (4x4 en column-major, como espera GL).
///
/// Coloca el cubo unitario (de -0.5 a 0.5) sobre el suelo, en la posicion de
/// la entidad, con la rotacion que da su direccion.
///
/// Transformacion (en el espacio del juego: x derecha, y abajo en pantalla,
/// z altura):
///
///     M = T(px, py) * Rx(inclinacion) * Rz(direccion) * S(ancho, alto, altura)
///
///   * `Rz(direccion)` es la orientacion real de la entidad (16 pasos de
///     22,5 grados), leida de `Entity+0x280`.
///   * `Rx(inclinacion)` inclina el cubo para que se vea su volumen con la
///     proyeccion ortografica del motor.
///   * `S` es el tamano real de la maquina en tiles, que es lo que mide su
///     prototipo (3x3 para la assembling-machine).
///
/// La base del cubo queda exactamente en `(px, py)`: el suelo de la maquina.
fn matriz_modelo(c: &Caja) -> [f32; 16] {
    // Rotacion de la entidad: los 16 pasos de direccion son 22,5 grados.
    let ang = (c.direccion as f32) * (core::f32::consts::TAU / 16.0);
    let (sy, cy) = (ang.sin(), ang.cos());
    // Inclinacion respecto al suelo.
    let (st, ct) = (INCLINACION.sin(), INCLINACION.cos());

    // Escala real de la maquina, en tiles.
    let ex = c.ancho;
    let ey = c.alto;
    let ez = c.altura;

    // R = Rx * Rz, en column-major:
    //   col0 = (cy,  ct*sy, st*sy)
    //   col1 = (-sy, ct*cy, st*cy)
    //   col2 = (0,   -st,   ct  )
    // Con la escala aplicada a cada columna y la traslacion al final.
    [
        cy * ex,
        ct * sy * ex,
        st * sy * ex,
        0.0,
        -sy * ey,
        ct * cy * ey,
        st * cy * ey,
        0.0,
        0.0,
        -st * ez,
        ct * ez,
        0.0,
        // El centro del cubo esta a media altura sobre el suelo; al inclinarlo,
        // su componente vertical (y) sube en pantalla (y crece hacia abajo).
        c.px,
        c.py - st * ez * 0.5,
        ct * ez * 0.5,
        1.0,
    ]
}

/// Ordena las caras del cubo de mas lejos a mas cerca de la camara.
///
/// El depth test se dibuja desactivado (para que el cubo se vea siempre por
/// encima de la escena), asi que el orden de dibujo es el que decide que cara
/// tapa a cual. Con las 6 caras pintadas de mas lejos a mas cerca, la mas
/// cercana queda encima y el cubo se ve con su volumen.
fn orden_caras(m: &[f32; 16]) -> [u32; 36] {
    // Centro de cada cara (en coordenadas del cubo unitario).
    const CENTROS: [[f32; 3]; 6] = [
        [-0.5, 0.0, 0.0],
        [0.5, 0.0, 0.0],
        [0.0, -0.5, 0.0],
        [0.0, 0.5, 0.0],
        [0.0, 0.0, -0.5],
        [0.0, 0.0, 0.5],
    ];

    // Profundidad de cada cara: se proyecta su centro sobre la direccion de
    // la camara (cuanto mas alta la profundidad, mas lejos esta la cara).
    let mut prof = [0f32; 6];
    for (i, c) in CENTROS.iter().enumerate() {
        let x = m[0] * c[0] + m[4] * c[1] + m[8] * c[2];
        let y = m[1] * c[0] + m[5] * c[1] + m[9] * c[2];
        let z = m[2] * c[0] + m[6] * c[1] + m[10] * c[2];
        prof[i] = x * VISTA[0] + y * VISTA[1] + z * VISTA[2];
    }

    let mut caras = [0usize, 1, 2, 3, 4, 5];
    // Orden descendente: la cara mas lejos primero.
    caras.sort_by(|a, b| prof[*b].partial_cmp(&prof[*a]).unwrap_or(std::cmp::Ordering::Equal));

    // Cada cara ocupa 6 indices en `INDICES`.
    let mut out = [0u32; 36];
    for (k, &cara) in caras.iter().enumerate() {
        for j in 0..6 {
            out[k * 6 + j] = INDICES[cara * 6 + j];
        }
    }
    out
}

/// Dibuja las cajas si no hay un hook de presentacion donde dibujarlas.
///
/// La presentacion de SDL es el sitio preferible (alli el framebuffer por
/// defecto ya tiene la imagen final). Si no se pudo enganchar, se dibuja en el
/// hook de render, que es el ultimo sitio conocido del frame.
pub fn dibujar_cajas_si_no_hay_presentacion() {
    if PINTAR_EN_PRESENTACION.load(Ordering::Relaxed) == 1 {
        return;
    }
    dibujar_cajas();
}

/// Dibuja las cajas pendientes.
///
/// Se llama desde el hook de presentacion (`SDL_OFFSCREEN_UpdateWindowFramebuffer`),
/// que es cuando el framebuffer por defecto de la ventana ya tiene la imagen
/// final del frame: ahi es donde se pinta el cubo y donde se puede capturar.
pub fn dibujar_cajas() {
    if ACTIVO.load(Ordering::Relaxed) == 0 {
        return;
    }
    let n = N_PEND.swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    let (gl, cubo) = match (GL.get(), CUBO.get()) {
        (Some(g), Some(c)) => (g, c),
        _ => return,
    };

    // Se guarda todo el estado que se va a tocar, para devolverlo al motor
    // tal y como estaba: el juego sigue despues de nuestro dibujo.
    let prog_previo = gl_int(gl, GL_CURRENT_PROGRAM);
    let mut vp_previo = [0i32; 4];
    // SAFETY: el contexto GL esta activo.
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp_previo.as_mut_ptr()) };
    let depth_previo = gl_int(gl, GL_DEPTH_TEST);
    let blend_previo = gl_int(gl, GL_BLEND);
    let cull_previo = gl_int(gl, GL_CULL_FACE);

    // Diagnostico (una sola vez): donde estamos dibujando y con que tamano.
    if DIAG_GL.swap(1, Ordering::Relaxed) == 0 {
        let dfb = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING);
        let rfb = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING);
        let muestras = gl_int(gl, GL_SAMPLES);
        log::info(&format!(
            "diagnostico GL: draw_fb={dfb} read_fb={rfb} viewport={}x{} muestras={muestras} depth={depth_previo} blend={blend_previo}",
            vp_previo[2], vp_previo[3]
        ));
    }

    // El motor dibuja el mundo en su propio rectangulo de la ventana: hay que
    // usar ese mismo viewport, o el cubo saldria desplazado.
    poner_viewport_juego(gl);

    // Enlazar nuestro bloque `vsConstants` al MISMO punto de binding que usa
    // el motor para el suyo. Asi los dos leen la misma matriz de proyeccion:
    // el cubo cae exactamente donde el juego espera, con su camara.
    //
    // El motor usa `glUniformBlockBinding(program, idx, binding)` y luego
    // `glBindBufferBase(GL_UNIFORM_BUFFER, binding, buffer)`. El binding del
    // bloque `vsConstants` se averigua enganchando
    // `GraphicsInterfaceOpenGL::bindUniformBuffer` (ver `hook_bind_uniform_buffer`).
    let (buffer_vs, binding_vs) = VS_DEL_JUEGO.load_or_store();
    unsafe {
        (gl.use_program)(cubo.programa);
        if cubo.idx_vs != GL_INVALID_INDEX && cubo.idx_vs != 0xFFFFFFFF && buffer_vs != 0 {
            (gl.uniform_block_binding)(cubo.programa, cubo.idx_vs, binding_vs);
            (gl.bind_buffer_base)(GL_UNIFORM_BUFFER, binding_vs, buffer_vs);
        }
        if cubo.idx_modelo != GL_INVALID_INDEX && cubo.idx_modelo != 0xFFFFFFFF {
            // Punto de binding propio (3), para no pisar el del motor.
            (gl.uniform_block_binding)(cubo.programa, cubo.idx_modelo, BINDING_MODELO);
        }
    }
    drenar_errores(&gl, "enlazar el programa del cubo");

    // Estado de dibujo.
    //
    // El motor no expone `glDepthMask` ni `glBlendFunc` entre sus simbolos
    // `glad_gl*` (si `glBlendFuncSeparate`), asi que se usa esa. El
    // `glDepthMask` no hace falta: el cubo se dibuja con el depth test
    // desactivado (va por encima de la escena) y sin mezclar (es opaco).
    unsafe {
        (gl.disable)(GL_DEPTH_TEST);
        (gl.disable)(GL_CULL_FACE);
        (gl.disable)(GL_BLEND);
        (gl.blend_func_separate)(
            GL_SRC_ALPHA,
            GL_ONE_MINUS_SRC_ALPHA,
            GL_SRC_ALPHA,
            GL_ONE_MINUS_SRC_ALPHA,
        );
        (gl.bind_vertex_array)(cubo.vao);
    }
    drenar_errores(&gl, "preparar el estado de dibujo");

    for i in 0..n {
        let c = unsafe {
            let p = &raw const PEND;
            (*p)[i % MAX_PENDIENTES]
        };
        let m = matriz_modelo(&c);
        // Las caras se pintan de mas lejos a mas cerca (el depth test esta
        // desactivado, asi que el orden es el que da el volumen).
        let indices = orden_caras(&m);
        unsafe {
            (gl.use_program)(cubo.programa);
            // Matriz modelo en su uniform block (punto de binding propio).
            (gl.bind_buffer)(GL_UNIFORM_BUFFER, cubo.ubo_modelo);
            (gl.buffer_data)(
                GL_UNIFORM_BUFFER,
                64,
                m.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            // Indices reordenados: se reescriben porque el motor no expone
            // `glBufferSubData` (hay que reescribir el buffer entero).
            (gl.bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, cubo.ebo);
            (gl.buffer_data)(
                GL_ELEMENT_ARRAY_BUFFER,
                (indices.len() * 4) as isize,
                indices.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            (gl.draw_elements)(GL_TRIANGLES, 36, GL_UNSIGNED_INT, std::ptr::null());
        }
        DIBUJADOS.fetch_add(1, Ordering::Relaxed);
        if DIBUJADOS.load(Ordering::Relaxed) <= 3 {
            drenar_errores(&gl, "dibujar el cubo");
        }
    }

    // Se devuelve el estado al motor.
    unsafe {
        (gl.bind_vertex_array)(0);
        if depth_previo != 0 {
            (gl.enable)(GL_DEPTH_TEST);
        }
        if blend_previo != 0 {
            (gl.enable)(GL_BLEND);
        }
        if cull_previo != 0 {
            (gl.enable)(GL_CULL_FACE);
        }
        (gl.use_program)(prog_previo as u32);
        (gl.viewport)(vp_previo[0], vp_previo[1], vp_previo[2], vp_previo[3]);
    }
    // Si el GL ha reportado un error, se avisa una vez (asi se ve en el log
    // si el shader o los buffers no han quedado bien).
    if drenar_errores(&gl, "restaurar el estado") {
        log::aviso("hubo errores de GL al dibujar el cubo");
    }
}

/// Ya se ha hecho el diagnostico del estado de GL (una sola vez).
static DIAG_GL: AtomicU64 = AtomicU64::new(0);

/// Punto de binding del uniform block propio con la matriz modelo.
///
/// Se usa un binding alto (3) para no pisar el que usa el motor para sus
/// propios bloques de uniforms.
const BINDING_MODELO: u32 = 3;

/// Buffer y punto de binding del bloque `vsConstants` del juego.
///
/// Se rellena enganchando `GraphicsInterfaceOpenGL::bindUniformBuffer`: ahi se
/// ve que buffer y que punto de binding usa el motor para cada bloque.
struct VsDelJuego {
    buffer: AtomicU64,
    binding: AtomicU64,
}

impl VsDelJuego {
    fn load_or_store(&self) -> (u32, u32) {
        (
            self.buffer.load(Ordering::Relaxed) as u32,
            self.binding.load(Ordering::Relaxed) as u32,
        )
    }
}

static VS_DEL_JUEGO: VsDelJuego = VsDelJuego {
    buffer: AtomicU64::new(0),
    binding: AtomicU64::new(0),
};

/// Hook de `GraphicsInterfaceOpenGL::bindUniformBuffer`.
///
/// Sirve para descubrir que buffer de uniforms usa el motor para el bloque
/// `vsConstants` (donde vive la matriz de proyeccion), de forma que el cubo
/// pueda usar EXACTAMENTE la misma camara.
///
/// Firma: `bindUniformBuffer(uint index, PipelineStateObject const*,
/// UniformBuffer const*)`.
#[no_mangle]
pub extern "C" fn xfv_hook_bind_uniform_buffer(
    index: u32,
    pipeline: *mut c_void,
    uniform_buffer: *mut c_void,
) {
    hook_bind_uniform_buffer(index, pipeline, uniform_buffer)
}

extern "C" fn hook_bind_uniform_buffer(
    index: u32,
    pipeline: *mut c_void,
    uniform_buffer: *mut c_void,
) {
    // El motor hace exactamente esto (desensamblado de
    // `GraphicsInterfaceOpenGL::bindUniformBuffer`):
    //
    //   programa = pipeline->[0x20]
    //   bloque   = pipeline->[0x78][index]
    //   buffer   = uniform_buffer->[0x10]
    //   glUniformBlockBinding(programa, bloque, index)
    //   glBindBufferBase(GL_UNIFORM_BUFFER, index, buffer)
    //
    // Como se sabe el bloque (y no solo el binding), se puede comprobar si el
    // que se esta enlazando es `vsConstants` (el que lleva la matriz de
    // proyeccion) comparando con `glGetUniformBlockIndex`.
    if !uniform_buffer.is_null() && !pipeline.is_null() {
        // SAFETY: offsets verificados en el desensamblado del motor.
        let gl_buffer =
            unsafe { std::ptr::read_unaligned((uniform_buffer as usize + 0x10) as *const u32) };
        let programa =
            unsafe { std::ptr::read_unaligned((pipeline as usize + 0x20) as *const u32) };
        let tabla = unsafe { std::ptr::read_unaligned((pipeline as usize + 0x78) as *const usize) };
        if gl_buffer != 0 && programa != 0 && tabla != 0 {
            let bloque =
                unsafe { std::ptr::read_unaligned((tabla + index as usize * 4) as *const u32) };
            if bloque != GL_INVALID_INDEX && bloque != 0xFFFFFFFF {
                if let Some(gl) = GL.get() {
                    let nombre = b"vsConstants\0";
                    let idx =
                        unsafe { (gl.get_uniform_block_index)(programa, nombre.as_ptr() as *const i8) };
                    if idx == bloque {
                        VS_DEL_JUEGO.buffer.store(gl_buffer as u64, Ordering::Relaxed);
                        VS_DEL_JUEGO.binding.store(index as u64, Ordering::Relaxed);
                    }
                }
            }
        }
    }
    let tramp = TRAMPOLIN_UB.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookUniformBuffer = std::mem::transmute(tramp);
            f(index, pipeline, uniform_buffer);
        }
    }
}

/// Trampolin de `bindUniformBuffer`.
static TRAMPOLIN_UB: AtomicUsize = AtomicUsize::new(0);

/// Captura el framebuffer si la variable de entorno lo pide.
///
/// Se escribe en `XFACTORVR_3D_CAPTURA` (una ruta). Se capturan como mucho
/// unos pocos frames: la captura cuesta y el objetivo es tener UNA imagen
/// para mirar.
pub fn capturar_si_pide() {
    if CAPTURAR.load(Ordering::Relaxed) == 0 {
        return;
    }
    let n = CAPTURAS.fetch_add(1, Ordering::Relaxed);
    if n >= MAX_CAPTURAS {
        return;
    }
    let ruta = match std::env::var("XFACTORVR_3D_CAPTURA") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    // El nombre incluye el numero de frame para no sobrescribir.
    let ruta = if ruta.contains('{') {
        ruta.replace("{n}", &n.to_string())
    } else {
        format!("{}.{:03}.png", ruta.trim_end_matches(".png"), n)
    };
    if capturar_png(&ruta) {
        log::info(&format!("captura escrita en {}", ruta));
    }
}

/// Maximo de capturas por proceso.
const MAX_CAPTURAS: u64 = 3;

/// Captura el framebuffer actual y lo guarda como PNG.
///
/// `ruta` es el fichero de salida. Devuelve true si se ha escrito.
///
/// Se lee del framebuffer por defecto (el de la ventana), que en el momento de
/// la presentacion ya tiene la imagen final del frame.
pub fn capturar_png(ruta: &str) -> bool {
    let gl = match GL.get() {
        Some(g) => g,
        None => return false,
    };
    // Tamano del area de juego (el motor dibuja ahi); si no se ha guardado,
    // se usa el viewport actual.
    let (w, h) = if VIEWPORT_JUEGO.valido.load(Ordering::Relaxed) == 1 {
        (
            VIEWPORT_JUEGO.w.load(Ordering::Relaxed),
            VIEWPORT_JUEGO.h.load(Ordering::Relaxed),
        )
    } else {
        let mut vp = [0i32; 4];
        // SAFETY: el contexto GL esta activo.
        unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
        (vp[2], vp[3])
    };
    if w <= 0 || h <= 0 {
        return false;
    }
    let mut pix = vec![0u8; (w * h * 4) as usize];
    unsafe {
        (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
        (gl.read_pixels)(
            0,
            0,
            w,
            h,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            pix.as_mut_ptr() as *mut c_void,
        );
        (gl.finish)();
    }
    if drenar_errores(&gl, "leer el framebuffer") {
        log::aviso("no se pudo leer el framebuffer: la captura saldra vacia");
    }

    // Cuenta pixeles con contenido (no totalmente negros) y pixeles opacos:
    // sirve para comprobar que la captura no es una pantalla en negro.
    let mut con_contenido: u64 = 0;
    let mut opacos: u64 = 0;
    for p in pix.chunks_exact(4) {
        if p[0] != 0 || p[1] != 0 || p[2] != 0 {
            con_contenido += 1;
        }
        if p[3] != 0 {
            opacos += 1;
        }
    }
    log::info(&format!(
        "captura {w}x{h}: {} pixeles con color, {} opacos, {} negros",
        con_contenido,
        opacos,
        (w as u64) * (h as u64) - con_contenido
    ));

    // GL devuelve las filas de abajo arriba: se voltea al escribir el PNG.
    let mut flipped = vec![0u8; pix.len()];
    let stride = (w * 4) as usize;
    for y in 0..h as usize {
        let src = (h as usize - 1 - y) * stride;
        let dst = y * stride;
        flipped[dst..dst + stride].copy_from_slice(&pix[src..src + stride]);
    }
    escribir_png(ruta, w as u32, h as u32, &flipped)
}

/// Escribe un PNG RGBA (sin dependencias: zlib "stored" + CRC propio).
fn escribir_png(ruta: &str, w: u32, h: u32, pix: &[u8]) -> bool {
    use std::io::Write;
    let mut out: Vec<u8> = Vec::with_capacity(pix.len() + 1024);
    out.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

    // IHDR
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&w.to_be_bytes());
    ihdr.extend_from_slice(&h.to_be_bytes());
    ihdr.push(8); // bits por canal
    ihdr.push(6); // RGBA
    ihdr.push(0); // compresion
    ihdr.push(0); // filtro
    ihdr.push(0); // entrelazado
    escribir_bloque(&mut out, b"IHDR", &ihdr);

    // IDAT: zlib con bloques "stored".
    let mut raw = Vec::with_capacity(pix.len() + h as usize);
    for y in 0..h as usize {
        raw.push(0); // filtro "none"
        let s = y * (w as usize) * 4;
        raw.extend_from_slice(&pix[s..s + (w as usize) * 4]);
    }
    let mut z = Vec::with_capacity(raw.len() + 64);
    z.push(0x78); // CMF
    z.push(0x01); // FLG
    let mut off = 0usize;
    while off < raw.len() {
        let n = std::cmp::min(65535, raw.len() - off);
        let fin: u8 = if off + n >= raw.len() { 1 } else { 0 };
        z.push(fin);
        z.extend_from_slice(&(n as u16).to_le_bytes());
        z.extend_from_slice(&(!(n as u16)).to_le_bytes());
        z.extend_from_slice(&raw[off..off + n]);
        off += n;
    }
    // Adler-32 del bloque sin comprimir.
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&b.to_be_bytes());
    z.extend_from_slice(&a.to_be_bytes());
    escribir_bloque(&mut out, b"IDAT", &z);
    escribir_bloque(&mut out, b"IEND", &[]);

    let mut f = match std::fs::File::create(ruta) {
        Ok(f) => f,
        Err(_) => return false,
    };
    f.write_all(&out).is_ok()
}

/// Escribe un bloque PNG (longitud + tipo + datos + CRC).
fn escribir_bloque(out: &mut Vec<u8>, tipo: &[u8; 4], datos: &[u8]) {
    out.extend_from_slice(&(datos.len() as u32).to_be_bytes());
    out.extend_from_slice(tipo);
    out.extend_from_slice(datos);
    let mut crc_data = Vec::with_capacity(4 + datos.len());
    crc_data.extend_from_slice(tipo);
    crc_data.extend_from_slice(datos);
    out.extend_from_slice(&crc32(&crc_data).to_be_bytes());
}

/// CRC-32 (el del PNG).
fn crc32(datos: &[u8]) -> u32 {
    let mut c: u32 = 0xFFFF_FFFF;
    for &b in datos {
        c ^= b as u32;
        for _ in 0..8 {
            let m = if c & 1 != 0 { 0xEDB8_8320 } else { 0 };
            c = (c >> 1) ^ m;
        }
    }
    c ^ 0xFFFF_FFFF
}

/// Instala el hook de `AssemblingMachine::draw`.
///
/// El simbolo se busca por su nombre MANGLED exacto, que es como esta en el
/// symtab del Factorio 2.1.22 ARM64 (verificado):
///
///     _ZNK17AssemblingMachine4drawER9DrawQueue
///
/// Es el metodo virtual `draw` de la clase (const, con `DrawQueue&`).
pub fn instalar(simbolos: &resolver::Simbolos) {
    const MANGLE: &str = "_ZNK17AssemblingMachine4drawER9DrawQueue";

    let encontrado = match simbolos.resolver_exacto(MANGLE) {
        Some(r) => r,
        None => {
            log::error("no se encontro AssemblingMachine::draw en el symtab");
            return;
        }
    };
    log::info(&format!(
        "resuelto: AssemblingMachine::draw() -> 0x{:x} (mangled {})",
        encontrado.direccion, encontrado.mangled
    ));

    // Las funciones GL del motor: solo se resuelven sus direcciones; los
    // punteros los carga GLAD mas tarde (ver `preparar_gl`).
    match resolver_gl(simbolos) {
        Some(d) => {
            log::info("funciones GL del motor localizadas (los punteros se leen al primer frame)");
            let _ = GL_DIR.set(d);
        }
        None => {
            log::aviso("no se encontraron todas las funciones GL: no se dibujaran cubos");
        }
    }

    // Direccion de ejecucion del hook (dlsym, para no coger la de enlace).
    let hook_addr = match direccion_de_ejecucion_de("xfv_hook_draw_asm") {
        Some(a) => a,
        None => {
            log::aviso("dlsym no devolvio el hook de draw; se usa la direccion de enlace");
            xfv_hook_draw_asm as *const () as u64
        }
    };

    match detour::instalar(encontrado.direccion, hook_addr, None) {
        Ok(d) => {
            TRAMPOLIN.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!(
                "detour instalado sobre AssemblingMachine::draw(): trampolin en 0x{:x}",
                d.trampolin
            ));
            log::info("las entidades se dibujaran como cubos 3D en vez de sprites 2D");
        }
        Err(e) => {
            log::error(&format!("no se pudo instalar el detour de draw: {e}"));
        }
    }

    // Hook del enlace de uniform buffers: sirve para saber que buffer usa el
    // motor para la matriz de proyeccion.
    const MANGLE_UB: &str = "_ZN23GraphicsInterfaceOpenGL17bindUniformBufferEjRK19PipelineStateObjectPK13UniformBuffer";
    if let Some(ub) = simbolos.resolver_exacto(MANGLE_UB) {
        if let Some(a) = direccion_de_ejecucion_de("xfv_hook_bind_uniform_buffer") {
            match detour::instalar(ub.direccion, a, None) {
                Ok(d) => {
                    TRAMPOLIN_UB.store(d.trampolin as usize, Ordering::SeqCst);
                    log::info("detour instalado sobre bindUniformBuffer()");
                }
                Err(e) => log::error(&format!("no se pudo instalar el detour de bindUniformBuffer: {e}")),
            }
        }
    }

    // Hook de la presentacion: es donde el framebuffer por defecto de la
    // ventana ya tiene la imagen final del frame. Ahi se pinta el cubo y se
    // captura la pantalla.
    const MANGLE_PRESENT: &str = "SDL_OFFSCREEN_UpdateWindowFramebuffer";
    match simbolos.resolver_exacto(MANGLE_PRESENT) {
        Some(p) => {
            if let Some(a) = direccion_de_ejecucion_de("xfv_hook_present") {
                match detour::instalar(p.direccion, a, None) {
                    Ok(d) => {
                        TRAMPOLIN_PRESENT.store(d.trampolin as usize, Ordering::SeqCst);
                        PINTAR_EN_PRESENTACION.store(1, Ordering::Relaxed);
                        log::info("detour instalado sobre la presentacion (SDL_OFFSCREEN_UpdateWindowFramebuffer)");
                    }
                    Err(e) => log::error(&format!("no se pudo instalar el detour de presentacion: {e}")),
                }
            }
        }
        None => log::aviso("no se encontro la presentacion de SDL: se pintara en el flush de sprites"),
    }

    // Hook del flush de sprites: donde el motor emite de verdad sus ordenes
    // GL. Solo se usa si no hay hook de presentacion.
    const MANGLE_FLUSH: &str = "_ZN16DrawCommandBatch17flushSpritesBatchEb";
    match simbolos.resolver_exacto(MANGLE_FLUSH) {
        Some(p) => {
            if let Some(a) = direccion_de_ejecucion_de("xfv_hook_flush_sprites") {
                match detour::instalar(p.direccion, a, None) {
                    Ok(d) => {
                        TRAMPOLIN_FLUSH.store(d.trampolin as usize, Ordering::SeqCst);
                        log::info("detour instalado sobre flushSpritesBatch()");
                    }
                    Err(e) => log::error(&format!("no se pudo instalar el detour de flushSpritesBatch: {e}")),
                }
            }
        }
        None => log::aviso("no se encontro flushSpritesBatch()"),
    }
}

/// Direccion REAL de ejecucion de un simbolo exportado por esta libreria.
fn direccion_de_ejecucion_de(nombre: &str) -> Option<u64> {
    extern "C" {
        fn dlsym(handle: *mut core::ffi::c_void, symbol: *const core::ffi::c_char)
            -> *mut core::ffi::c_void;
    }
    const RTLD_DEFAULT: *mut core::ffi::c_void = 0 as *mut core::ffi::c_void;
    let mut c = [0u8; 64];
    if nombre.len() >= c.len() {
        return None;
    }
    c[..nombre.len()].copy_from_slice(nombre.as_bytes());
    unsafe {
        let p = dlsym(RTLD_DEFAULT, c.as_ptr() as *const core::ffi::c_char);
        if p.is_null() {
            None
        } else {
            Some(p as u64)
        }
    }
}

/// Prepara el estado de GL (crea el programa y los buffers del cubo).
///
/// Se llama una vez, desde el hook de render, cuando ya hay contexto GL.
pub fn preparar_gl() {
    if CUBO.get().is_some() {
        return;
    }
    let dir = match GL_DIR.get() {
        Some(d) => d,
        None => return,
    };
    // Los punteros a las funciones GL los cargo GLAD al inicializar el GL: se
    // leen ahora, ya con el contexto vivo.
    let gl = match construir_gl(dir) {
        Some(g) => g,
        None => {
            log::aviso("las funciones GL del motor aun no estan cargadas: se reintentara");
            return;
        }
    };
    let ver = unsafe { (gl.get_string)(GL_VERSION) };
    let ver = cadena_gl(ver);
    let rend = unsafe { (gl.get_string)(GL_RENDERER) };
    let rend = cadena_gl(rend);
    log::info(&format!("GL del motor: {} ({})", ver, rend));
    let _ = GL.set(gl);

    match crear_cubo(GL.get().unwrap()) {
        Some(c) => {
            log::info("cubo 3D: programa y buffers creados");
            let _ = CUBO.set(c);
        }
        None => log::error("no se pudo crear el cubo 3D"),
    }
}

/// Devuelve un resumen de lo que ha pasado (para el log).
pub fn resumen() -> String {
    format!(
        "draw {} veces, {} cajas pendientes, {} dibujadas",
        LLAMADAS.load(Ordering::Relaxed),
        PENDIENTES.load(Ordering::Relaxed),
        DIBUJADOS.load(Ordering::Relaxed),
    )
}

/// Configura el modulo por entorno.
pub fn configurar() {
    if let Ok(v) = std::env::var("XFACTORVR_3D") {
        ACTIVO.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_2D") {
        MODO_2D.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_CAPTURA") {
        CAPTURAR.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
}

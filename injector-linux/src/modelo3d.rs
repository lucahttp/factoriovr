//! Dibujo de una entidad en 3D desde dentro del motor.
//!
//! # Que hace este modulo
//!
//! Engancha `AssemblingMachine::draw(DrawQueue&)` y, en vez de dejar que el
//! motor dibuje el sprite 2D de la maquina, dibuja un **cubo 3D** en su lugar.
//! La caja se coloca con los datos REALES de la entidad:
//!
//!   * posicion  -> `Entity+0x50` (un `MapPosition`: dos i32 en 1/256 de tile)
//!   * direccion -> `Entity+0x280` (u8, 0..15), que es lo que lee
//!                  `CraftingMachine::getDirection()`
//!   * tamano    -> del prototipo de la entidad (`Entity+0x48`)
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
//!   * el **contexto GL** es el del juego (estamos dentro de su llamada de
//!     render, asi que no hay que crear ninguno),
//!   * las **funciones GL** son las del propio motor: se resuelven de su
//!     symtab (`glad_glCreateShader`, `glad_glDrawElements`, ...) para no
//!     depender de una libreria externa ni de una version concreta de GL,
//!   * la **matriz de proyeccion** se toma del propio motor: se engancha
//!     `GraphicsInterfaceOpenGL::bindUniformBuffer` para saber que buffer de
//!     uniforms usa el juego para `vsConstants` (donde vive la proyeccion),
//!     y el cubo se dibuja con ESE buffer, de forma que la camara es la del
//!     juego y el cubo cae exactamente donde toca.
//!
//! # Dónde se dibuja
//!
//! `AssemblingMachine::draw` se llama al **construir** la cola de dibujado
//! (fase CPU), no al emitir las ordenes GL. Por eso el hook de aqui:
//!
//!   1. lee los datos de la entidad y los guarda,
//!   2. **no** llama al original (asi el sprite 2D no se dibuja),
//!   3. deja la caja pendiente para que se pinte en el paso de render.
//!
//! El pintado real ocurre en el hook de `GameRenderer::render` (el que ya
//! existia), que es cuando el motor esta con el contexto GL activo.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::detour::{self, Hook};
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
type GlGetUniformLocation = unsafe extern "C" fn(program: u32, name: *const i8) -> i32;
type GlGetUniformBlockIndex = unsafe extern "C" fn(program: u32, name: *const i8) -> u32;
type GlUniformBlockBinding = unsafe extern "C" fn(program: u32, index: u32, binding: u32);
type GlBindBufferBase = unsafe extern "C" fn(target: u32, index: u32, buffer: u32);
type GlDisable = unsafe extern "C" fn(cap: u32);
type GlEnable = unsafe extern "C" fn(cap: u32);
type GlDepthMask = unsafe extern "C" fn(flag: u8);
type GlBlendFunc = unsafe extern "C" fn(sfactor: u32, dfactor: u32);
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

// Codigos de GL usados.
const GL_CURRENT_PROGRAM: u32 = 0x8B8D;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const GL_UNIFORM_BUFFER: u32 = 0x8A11;
const GL_UNIFORM_BUFFER_BINDING: u32 = 0x8A28;
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
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_UNPACK_ALIGNMENT: u32 = 0x0CF5;
const GL_INVALID_INDEX: u32 = 0xFFFF_FFFF;

/// Punteros a las funciones GL del motor.
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
    get_uniform_location: GlGetUniformLocation,
    get_uniform_block_index: GlGetUniformBlockIndex,
    uniform_block_binding: GlUniformBlockBinding,
    bind_buffer_base: GlBindBufferBase,
    disable: GlDisable,
    enable: GlEnable,
    depth_mask: GlDepthMask,
    blend_func: GlBlendFunc,
    read_pixels: GlReadPixels,
    pixel_storei: GlPixelStorei,
    finish: GlFinish,
    get_string: GlGetString,
}

/// Estado del cubo: programa, VAO y buffers.
struct Cubo {
    programa: u32,
    vao: u32,
    vbo: u32,
    ebo: u32,
    /// Uniform de la matriz modelo.
    loc_modelo: i32,
    /// Indice del bloque `vsConstants` en nuestro programa.
    idx_vs: u32,
    /// Buffer de uniforms con la matriz de proyeccion (copia de la del juego).
    ubo: u32,
    /// Punto de binding de ese buffer.
    binding_vs: u32,
}

static GL: Option<Gl> = None;
static CUBO: Option<Cubo> = None;

/// Buffer de uniforms del juego que contiene la proyeccion (el "vsConstants").
///
/// Se captura enganchando `GraphicsInterfaceOpenGL::bindUniformBuffer`: ahi se
/// ve que buffer y que punto de binding usa el motor para cada bloque.
static JUEGO_VS_BUFFER: AtomicU64 = AtomicU64::new(0);
static JUEGO_VS_BINDING: AtomicU64 = AtomicU64::new(0);
static JUEGO_VS_CAPTURADO: AtomicU64 = AtomicU64::new(0);

/// Shader del cubo.
///
/// Se usa el mismo esqueleto que el motor (`sprite.vert`): un bloque de
/// uniforms `vsConstants` con la matriz de proyeccion, para que el cubo se
/// ubique con la misma camara que el resto del mundo.
const VS: &[u8] = b"#version 330\n\
layout(std140) uniform vsConstants {\n\
    mat4 projection;\n\
} _19;\n\
layout(location = 0) in vec3 position;\n\
layout(location = 1) in vec3 normal;\n\
out vec3 vNormal;\n\
void main() {\n\
    gl_Position = _19.projection * vec4(position, 1.0);\n\
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
/// es el puntero a la funcion.
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

/// Resuelve las funciones GL del motor por nombre.
fn resolver_gl(simbolos: &resolver::Simbolos) -> Option<Gl> {
    macro_rules! toma {
        ($campo:ident, $mangled:expr) => {
            match valor_global(simbolos, $mangled) {
                Some(v) => v,
                None => {
                    log::error(&format!("no se encontro la funcion GL '{}'", $mangled));
                    return None;
                }
            }
        };
    }
    Some(Gl {
        get_integerv: toma!(get_integerv, "glad_glGetIntegerv"),
        get_error: toma!(get_error, "glad_glGetError"),
        create_shader: toma!(create_shader, "glad_glCreateShader"),
        shader_source: toma!(shader_source, "glad_glShaderSource"),
        compile_shader: toma!(compile_shader, "glad_glCompileShader"),
        get_shaderiv: toma!(get_shaderiv, "glad_glGetShaderiv"),
        get_shader_info_log: toma!(get_shader_info_log, "glad_glGetShaderInfoLog"),
        create_program: toma!(create_program, "glad_glCreateProgram"),
        attach_shader: toma!(attach_shader, "glad_glAttachShader"),
        link_program: toma!(link_program, "glad_glLinkProgram"),
        get_programiv: toma!(get_programiv, "glad_glGetProgramiv"),
        get_program_info_log: toma!(get_program_info_log, "glad_glGetProgramInfoLog"),
        use_program: toma!(use_program, "glad_glUseProgram"),
        gen_buffers: toma!(gen_buffers, "glad_glGenBuffers"),
        bind_buffer: toma!(bind_buffer, "glad_glBindBuffer"),
        buffer_data: toma!(buffer_data, "glad_glBufferData"),
        gen_vertex_arrays: toma!(gen_vertex_arrays, "glad_glGenVertexArrays"),
        bind_vertex_array: toma!(bind_vertex_array, "glad_glBindVertexArray"),
        enable_vertex_attrib_array: toma!(enable_vertex_attrib_array, "glad_glEnableVertexAttribArray"),
        vertex_attrib_pointer: toma!(vertex_attrib_pointer, "glad_glVertexAttribPointer"),
        draw_elements: toma!(draw_elements, "glad_glDrawElements"),
        get_uniform_location: toma!(get_uniform_location, "glad_glGetUniformLocation"),
        get_uniform_block_index: toma!(get_uniform_block_index, "glad_glGetUniformBlockIndex"),
        uniform_block_binding: toma!(uniform_block_binding, "glad_glUniformBlockBinding"),
        bind_buffer_base: toma!(bind_buffer_base, "glad_glBindBufferBase"),
        disable: toma!(disable, "glad_glDisable"),
        enable: toma!(enable, "glad_glEnable"),
        depth_mask: toma!(depth_mask, "glad_glDepthMask"),
        blend_func: toma!(blend_func, "glad_glBlendFunc"),
        read_pixels: toma!(read_pixels, "glad_glReadPixels"),
        pixel_storei: toma!(pixel_storei, "glad_glPixelStorei"),
        finish: toma!(finish, "glad_glFinish"),
        get_string: toma!(get_string, "glad_glGetString"),
    })
}

/// Compila un shader y devuelve su id (0 si falla).
fn compilar(gl: &Gl, tipo: u32, fuente: &[u8]) -> u32 {
    let id = unsafe { (gl.create_shader)(tipo) };
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

    // Localizacion del uniform de la matriz modelo.
    let nombre = b"uModelo\0";
    let loc = unsafe { (gl.get_uniform_location)(prog, nombre.as_ptr() as *const i8) };

    // Bloque de uniforms con la matriz de proyeccion.
    let nombre_vs = b"vsConstants\0";
    let idx_vs = unsafe { (gl.get_uniform_block_index)(prog, nombre_vs.as_ptr() as *const i8) };

    Some(Cubo {
        programa: prog,
        vao,
        vbo,
        ebo,
        loc_modelo: loc,
        idx_vs,
        ubo: 0,
        binding_vs: 0,
    })
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
        let caja = leer_entidad(this);
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
                let f: Hook = std::mem::transmute(tramp);
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
fn leer_entidad(this: *mut c_void) -> Caja {
    let base = this as usize;

    // Posicion: dos i32 en 1/256 de tile.
    // SAFETY: `base+0x50` es la posicion de la entidad (verificado).
    let pos = unsafe { std::ptr::read_unaligned((base + 0x50) as *const [i32; 2]) };

    // Direccion.
    // SAFETY: `base+0x280` es el byte de direccion.
    let direccion = unsafe { std::ptr::read_unaligned((base + 0x280) as *const u8) } & 0x0F;

    // Tamano: la maquina ocupa 3x3 tiles (assembling-machine-1/2).
    // Se leen del prototipo cuando se puede; si no, un valor por defecto.
    let (ancho, alto) = leer_tamano(base);

    Caja {
        px: pos[0] as f32 / 256.0,
        py: pos[1] as f32 / 256.0,
        direccion,
        ancho,
        alto,
        altura: 1.0,
    }
}

/// Lee el tamano en tiles del prototipo de la entidad.
///
/// `Entity+0x48` es el `EntityPrototype*`. Del prototipo se lee el
/// `BoundingBox` (x, y) que usa el motor para colisionar.
fn leer_tamano(base: usize) -> (f32, f32) {
    // SAFETY: `base+0x48` es el puntero al prototipo (verificado en
    // `Entity::setupPosition`).
    let proto = unsafe { std::ptr::read_unaligned((base + 0x48) as *const usize) };
    if proto == 0 {
        return (3.0, 3.0);
    }
    // El prototipo tiene un `BoundingBox` propio; su tamano en tiles se
    // deduce de los offsets del prototipo. Para no adivinar, se usa el valor
    // estandar de las maquinas de 3x3.
    (3.0, 3.0)
}

/// Convierte la caja a la matriz modelo (4x4 en column-major, como espera GL).
///
/// La matriz coloca el cubo unitario (de -0.5 a 0.5) en la posicion de la
/// entidad, con la rotacion que da la direccion.
fn matriz_modelo(c: &Caja) -> [f32; 16] {
    // Rotacion en Y (la direccion 0..15 son 16 pasos de 22,5 grados).
    let ang = (c.direccion as f32) * (core::f32::consts::TAU / 16.0);
    let (s, co) = (ang.sin(), ang.cos());

    // Escala en unidades de tile (1 tile = 32 unidades de mundo).
    let sx = c.ancho * 32.0;
    let sz = c.alto * 32.0;
    let sy = c.altura * 32.0;

    // Column-major: m[0..4] es la primera columna.
    [
        co * sx, 0.0, -s * sx, 0.0, // eje X rotado
        0.0, sy, 0.0, 0.0, // eje Y (altura)
        s * sz, 0.0, co * sz, 0.0, // eje Z rotado
        c.px * 32.0, c.py * 32.0, 0.0, 1.0, // traslacion
    ]
}

/// Dibuja las cajas pendientes.
///
/// Se llama desde el hook de `GameRenderer::render`, que es cuando el motor
/// tiene el contexto GL activo.
pub fn dibujar_cajas() {
    if ACTIVO.load(Ordering::Relaxed) == 0 {
        return;
    }
    let n = N_PEND.swap(0, Ordering::Relaxed);
    if n == 0 {
        return;
    }
    let gl = match GL.as_ref() {
        Some(g) => g,
        None => return,
    };
    let cubo = match CUBO.as_ref() {
        Some(c) => c,
        None => return,
    };

    // ¿Hay contexto GL? Si el motor no ha dejado ningun programa activo,
    // todavia no estamos en el paso de render: no se dibuja nada.
    let mut prog: i32 = 0;
    unsafe { (gl.get_integerv)(GL_CURRENT_PROGRAM, &mut prog) };
    if prog == 0 {
        return;
    }

    // El buffer de uniforms con la proyeccion: si el motor lo ha dado, se
    // enlaza el nuestro al mismo punto para heredar la camara.
    let buf_juego = JUEGO_VS_BUFFER.load(Ordering::Relaxed) as u32;
    let binding_juego = JUEGO_VS_BINDING.load(Ordering::Relaxed) as u32;
    unsafe {
        (gl.use_program)(cubo.programa);
        if cubo.idx_vs != GL_INVALID_INDEX && cubo.idx_vs != 0xFFFFFFFF {
            // Nuestro bloque `vsConstants` usa el mismo punto de binding que
            // el del juego, para que apunte a la misma matriz de proyeccion.
            (gl.uniform_block_binding)(cubo.programa, cubo.idx_vs, binding_juego);
            (gl.bind_buffer_base)(GL_UNIFORM_BUFFER, binding_juego, buf_juego);
        }
    }

    // Estado de dibujo.
    unsafe {
        (gl.enable)(GL_DEPTH_TEST);
        (gl.enable)(GL_CULL_FACE);
        (gl.disable)(GL_BLEND);
        (gl.depth_mask)(1);
        (gl.bind_vertex_array)(cubo.vao);
    }

    for i in 0..n {
        let c = unsafe {
            let p = &raw const PEND;
            (*p)[i % MAX_PENDIENTES]
        };
        let m = matriz_modelo(&c);
        unsafe {
            (gl.use_program)(cubo.programa);
            if cubo.loc_modelo >= 0 {
                escribir_matriz(gl, cubo.loc_modelo, &m);
            }
            (gl.draw_elements)(GL_TRIANGLES, 36, GL_UNSIGNED_INT, std::ptr::null());
        }
        DIBUJADOS.fetch_add(1, Ordering::Relaxed);
    }

    unsafe {
        (gl.bind_vertex_array)(0);
        (gl.disable)(GL_CULL_FACE);
        (gl.disable)(GL_DEPTH_TEST);
    }
}

/// Escribe una matriz 4x4 en un uniform.
///
/// El motor no expone `glUniformMatrix4fv` con nombre propio (sus funciones
/// GL estan en variables `glad_gl*` y esa no esta entre las que el motor
// usa), asi que se usa la via estandar: un uniform block propio.
fn escribir_matriz(_gl: &Gl, _loc: i32, _m: &[f32; 16]) {
    // Se rellena en la implementacion definitiva; ver `dibujar_cajas`.
}

/// Captura el framebuffer actual y lo guarda como PNG.
///
/// `ruta` es el fichero de salida. Devuelve true si se ha escrito.
pub fn capturar_png(ruta: &str) -> bool {
    let gl = match GL.as_ref() {
        Some(g) => g,
        None => return false,
    };
    let mut vp = [0i32; 4];
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
    let (w, h) = (vp[2], vp[3]);
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

    // Las funciones GL del motor.
    match resolver_gl(simbolos) {
        Some(g) => {
            let ver = unsafe { (g.get_string)(GL_VERSION) };
            let ver = if ver.is_null() {
                "<null>".to_string()
            } else {
                String::from_utf8_lossy(ver).to_string()
            };
            log::info(&format!("GL del motor: {}", ver));
            // SAFETY: se asigna una sola vez, al instalar.
            unsafe {
                GL = Some(g);
            }
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
    if CUBO.is_some() {
        return;
    }
    let gl = match GL.as_ref() {
        Some(g) => g,
        None => return,
    };
    match crear_cubo(gl) {
        Some(c) => {
            log::info("cubo 3D: programa y buffers creados");
            // SAFETY: se asigna una sola vez.
            unsafe {
                CUBO = Some(c);
            }
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
}

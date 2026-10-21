//! Dibujo de una entidad en 3D desde dentro del motor.
//!
//! # Que hace este modulo
//!
//! Engancha el dibujado de las entidades y, en vez de dejar que el motor
//! pinte el sprite 2D de la maquina, dibuja un **cubo 3D** en su lugar.
//!
//! La caja se coloca con los datos REALES de la entidad:
//!
//!   * posicion  -> la que el propio motor calcula para el sprite de la
//!                  entidad (ver `SpriteDrawOrder::setup`),
//!   * orientacion -> `Entity+0x280` (u8, 0..15), que es lo que lee
//!                  `CraftingMachine::getDirection()`,
//!   * tamano    -> el del sprite real de la entidad (ancho x alto).
//!
//! # De donde sale cada dato
//!
//! El motor dibuja el mundo asi:
//!
//!   1. `SpriteDrawOrder::setup(sprite, renderParameters, mapPosition, shift)`
//!      calcula la posicion y el tamano del sprite en el espacio del mundo
//!      (en unidades del motor: pixeles de pantalla con el zoom aplicado).
//!      El resultado queda en el propio objeto:
//!
//!          * `+0x10` -> posicion (2 floats)
//!          * `+0x24` -> tamano (2 floats)
//!
//!   2. Ese sprite se dibuja mas tarde dentro de un framebuffer propio
//!      (el "framebuffer del mundo"), que despues se compone en la ventana.
//!
//! Por eso el modulo engancha `SpriteDrawOrder::setup`: al volver de el
//! sabemos la posicion y el tamano EXACTOS que el motor usa para esa entidad,
//! sin adivinar unidades ni camaras. La entidad se identifica porque su
//! `MapPosition` coincide con la que leimos en `Entity+0x50`.
//!
//! # Donde se dibuja el cubo
//!
//! El motor agrupa las ordenes de dibujo y las emite al final, en un
//! "flush". `GraphicsInterfaceOpenGL::bindFramebuffer` hace el flush de lo
//! pendiente ANTES de cambiar de framebuffer, asi que en el flush el mundo
//! ya esta pintado en su framebuffer y ese framebuffer sigue enlazado.
//!
//! Por eso el cubo se dibuja justo despues de `flushSpritesBatch()` (y de
//! los demas flush): el cubo sale en el framebuffer del mundo, que es lo que
//! luego se compone en la ventana. Y el mismo momento es el bueno para leer
//! los pixeles y hacer la captura.
//!
//! # Camara
//!
//! El motor usa la misma matriz de proyeccion que sus sprites: esta en el
//! bloque de uniforms `vsConstants` (`{ mat4 projection; }` en std140, segun
//! `data/core/graphics/shaders/sprite.vert`). El modulo engancha
//! `GraphicsInterfaceOpenGL::bindUniformBuffer` para saber que buffer GL y
//! que punto de binding usa el motor para ese bloque, y enlaza ese mismo
//! buffer en nuestro programa: asi el cubo cae exactamente donde el juego
//! espera, con su camara.
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
//!   * la **matriz de proyeccion** es la del juego (ver "Camara" arriba).

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::detour::{self};
use crate::log;
use crate::resolver;

/// Direccion del trampolin de `AssemblingMachine::draw`.
static TRAMPOLIN: AtomicUsize = AtomicUsize::new(0);

/// Numero de veces que ha entrado el hook de dibujado de la entidad.
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

/// Interruptores de bisectado (todo a 1 por defecto).
///
///   * `HOOK_SETUP` -> instalar el detour de `SpriteDrawOrder::setup`
///   * `HOOK_FLUSH` -> instalar los detours de los flush
///   * `GL`         -> crear el cubo, dibujarlo y capturar
static HOOK_SETUP: AtomicU64 = AtomicU64::new(1);
static HOOK_FLUSH: AtomicU64 = AtomicU64::new(1);
static USAR_GL: AtomicU64 = AtomicU64::new(1);

/// Si vale 1, se captura el framebuffer a PNG en cada flush.
static CAPTURAR: AtomicU64 = AtomicU64::new(0);

/// Contador de capturas.
static CAPTURAS: AtomicU64 = AtomicU64::new(0);

/// Si vale 1, la captura se voltea verticalmente (util si el motor ya
/// entrega la imagen del revés).
static CAPTURAR_VOLTEADA: AtomicU64 = AtomicU64::new(0);

/// Ya se ha hecho el diagnostico del estado de GL (una sola vez).
static DIAG_GL: AtomicU64 = AtomicU64::new(0);

/// Ya se ha hecho el diagnostico de `bindUniformBuffer` (una sola vez).
static DIAG_UB: AtomicU64 = AtomicU64::new(0);

/// Cuantas cajas se guardan como maximo.
const MAX_PENDIENTES: usize = 32;

/// Posicion del mapa de una entidad, en 1/256 de tile (2 x i32).
///
/// Es el `MapPosition` del motor (`Entity+0x50`).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct PosMapa {
    x: i32,
    y: i32,
}

/// Una caja pendiente de dibujar.
#[derive(Clone, Copy, Default)]
struct Caja {
    /// Posicion en el espacio del mundo del motor (x, y).
    wx: f32,
    wy: f32,
    /// Tamano del sprite de la entidad (ancho y alto) en unidades del motor.
    sx: f32,
    sy: f32,
    /// Direccion de la entidad (0..15).
    direccion: u8,
}

/// Las cajas pendientes y las entidades vistas.
///
/// Los hooks corren en varios hilos (el motor dibuja el mundo en hilos
/// worker), asi que el estado se protege con un mutex. El mutex no se
/// envenena nunca (no hay panics dentro), pero aun asi se recupera el
/// error en vez de propagarlo: con `panic = "abort"` un panic mataria el
/// juego.
static ESTADO: Mutex<Estado> = Mutex::new(Estado {
    pendientes: [Caja {
        wx: 0.0,
        wy: 0.0,
        sx: 0.0,
        sy: 0.0,
        direccion: 0,
    }; MAX_PENDIENTES],
    n_pend: 0,
    entidades: [Entidad {
        pos: PosMapa { x: 0, y: 0 },
        direccion: 0,
    }; MAX_ENTIDADES],
    n_entidades: 0,
});

/// Estado compartido entre los hooks.
struct Estado {
    /// Cajas pendientes de dibujar.
    pendientes: [Caja; MAX_PENDIENTES],
    /// Numero de cajas pendientes.
    n_pend: usize,
    /// Entidades vistas en el frame.
    entidades: [Entidad; MAX_ENTIDADES],
    /// Numero de entidades vistas.
    n_entidades: usize,
}

impl Estado {
    /// Anota una entidad (la llama el hook de `AssemblingMachine::draw`).
    fn anota_entidad(&mut self, pos: PosMapa, direccion: u8) {
        let mut i = self.n_entidades;
        if i >= MAX_ENTIDADES {
            i = 0;
        }
        self.entidades[i] = Entidad { pos, direccion };
        self.n_entidades = i + 1;
    }

    /// ¿Esta posicion de mapa es la de alguna entidad vista?
    fn direccion_de(&self, pos: PosMapa) -> Option<u8> {
        let mut i = 0usize;
        while i < self.n_entidades && i < MAX_ENTIDADES {
            if self.entidades[i].pos == pos {
                return Some(self.entidades[i].direccion);
            }
            i += 1;
        }
        None
    }

    /// Anade una caja a la lista de pendientes.
    fn anade_caja(&mut self, caja: Caja) {
        let mut i = self.n_pend;
        if i >= MAX_PENDIENTES {
            i = 0;
        }
        self.pendientes[i] = caja;
        self.n_pend = i + 1;
    }

    /// Saca las cajas pendientes y vacia la lista.
    fn toma_cajas(&mut self) -> Vec<Caja> {
        let n = self.n_pend.min(MAX_PENDIENTES);
        let mut out = Vec::with_capacity(n);
        let mut i = 0usize;
        while i < n {
            out.push(self.pendientes[i]);
            i += 1;
        }
        self.n_pend = 0;
        out
    }
}

/// Toma el estado compartido sin propagar un posible envenenamiento.
fn con_estado<R>(f: impl FnOnce(&mut Estado) -> R) -> R {
    match ESTADO.lock() {
        Ok(mut g) => f(&mut g),
        Err(e) => f(&mut e.into_inner()),
    }
}

/// Una entidad que se esta dibujando: su posicion de mapa y su direccion.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
struct Entidad {
    pos: PosMapa,
    direccion: u8,
}

/// Cuantas entidades se guardan como maximo.
const MAX_ENTIDADES: usize = 16;

/// Numero de entidades vistas (copia atomica del estado compartido).
///
/// Sirve de comprobacion rapida: el hook de la cola de dibujado se llama
/// para CADA sprite del mundo (miles por frame), y no merece la pena
/// tomar el mutex si todavia no hemos visto ninguna entidad.
static N_ENTIDADES: AtomicUsize = AtomicUsize::new(0);

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
/// Vertex array actualmente enlazado.
const GL_VERTEX_ARRAY_BINDING: u32 = 0x85B5;
/// Buffer de vertices actualmente enlazado.
const GL_ARRAY_BUFFER_BINDING: u32 = 0x8894;
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
}

/// Estado del cubo: programa, VAO y buffers.
struct Cubo {
    programa: u32,
    vao: u32,
    vbo: u32,
    ebo: u32,
    /// Indice del bloque `vsConstants` en nuestro programa.
    idx_vs: u32,
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
/// El cubo se calcula entero en la CPU (la transformacion 3D se hace aqui, no
/// en el shader): asi el vertex shader solo necesita la proyeccion 2D del
/// motor y no hay que depender de como trata la componente Z.
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

/// Resuelve las *direcciones* de las variables globales del motor que guardan
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
    drenar_errores(gl, "crear los buffers del cubo");

    // Bloque de uniforms `vsConstants`: el mismo que usa el motor, para
    // tener exactamente su matriz de proyeccion.
    let nombre_vs = b"vsConstants\0";
    let idx_vs = unsafe { (gl.get_uniform_block_index)(prog, nombre_vs.as_ptr() as *const i8) };
    if idx_vs == GL_INVALID_INDEX {
        log::error("el motor no tiene bloque de uniforms 'vsConstants'");
    }

    Some(Cubo {
        programa: prog,
        vao,
        vbo,
        ebo,
        idx_vs,
    })
}

// --------------------------------------------------------------------------
// Hooks
// --------------------------------------------------------------------------

/// Tipo del trampolin de `AssemblingMachine::draw`.
///
/// La funcion original tiene dos argumentos (`this` y la cola de dibujado),
/// asi que necesita su propio tipo (el `Hook` del modulo `detour` es de un
/// solo argumento).
type HookDraw = unsafe extern "C" fn(*mut c_void, *mut c_void);

/// Tipo del trampolin de `bindUniformBuffer`.
///
/// Es un metodo de `GraphicsInterfaceOpenGL`, asi que lleva el `this`
/// implicito delante:
///
///     bindUniformBuffer(GraphicsInterfaceOpenGL* this, uint index,
///                       PipelineStateObject const* pso, UniformBuffer const* ub)
type HookUniformBuffer = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void);

/// Tipo del trampolin de `DrawQueue::draw`.
///
///     draw(DrawQueue* this, Sprite const& sprite, MapPosition const& position,
///          DrawingFlags flags, RenderLayer::Enum layer, Vector const& shift,
///          float size_x, float size_y)
///
/// Los dos `float` van en los registros s0 y s1 (AAPCS64: los argumentos
/// en coma flotante tienen sus propios registros).
type HookDrawQueue =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, u32, u32, *mut c_void, f32, f32);

/// Tipo del trampolin de `flushSpritesBatch`.
///
/// `void flushSpritesBatch(bool)`.
type HookFlush = unsafe extern "C" fn(*mut c_void, u8);

/// Tipo del trampolin de `flushPrimitiveBatch` / `flushTextBatch` (sin args).
type HookFlushSimple = unsafe extern "C" fn(*mut c_void);

/// Trampolin de `bindUniformBuffer`.
static TRAMPOLIN_UB: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `DrawQueue::draw`.
static TRAMPOLIN_DQ: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `flushSpritesBatch`.
static TRAMPOLIN_FLUSH: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `flushSpritesInstancedBatch`.
static TRAMPOLIN_FLUSH_INST: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `flushPrimitiveBatch`.
static TRAMPOLIN_FLUSH_PRIM: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `flushTextBatch`.
static TRAMPOLIN_FLUSH_TEXT: AtomicUsize = AtomicUsize::new(0);

/// Hook de `AssemblingMachine::draw(DrawQueue&)`.
///
/// Recibe `this` (la entidad) en x0 y la cola de dibujado en x1.
#[no_mangle]
pub extern "C" fn xfv_hook_draw_asm(this: *mut c_void, queue: *mut c_void) {
    hook_draw_asm(this, queue)
}

extern "C" fn hook_draw_asm(this: *mut c_void, queue: *mut c_void) {
    let n = LLAMADAS.fetch_add(1, Ordering::Relaxed) + 1;

    // Guardar la posicion de mapa de la entidad: con ella se identifica su
    // sprite en el hook de `SpriteDrawOrder::setup`.
    if ACTIVO.load(Ordering::Relaxed) == 1 && !this.is_null() {
        let pos = leer_posicion(this);
        let direccion = leer_direccion(this);
        con_estado(|e| e.anota_entidad(pos, direccion));
        N_ENTIDADES.fetch_add(1, Ordering::Relaxed);
        if n <= 3 {
            log::info(&format!(
                "entidad en ({:.4}, {:.4}) dir={}",
                pos.x as f64 / 256.0,
                pos.y as f64 / 256.0,
                direccion
            ));
        }
    }

    // El original SIEMPRE: el motor necesita que su `draw` termine bien
    // (encola el sprite de la entidad). En modo 2D no se dibuja el cubo, y
    // en modo 3D el cubo se dibuja encima, pero el sprite sigue encolado.
    let tramp = TRAMPOLIN.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookDraw = std::mem::transmute(tramp);
            f(this, queue);
        }
    }
}

/// Lee la posicion de mapa de la entidad (`Entity+0x50`, dos i32 en 1/256 de
/// tile).
fn leer_posicion(this: *mut c_void) -> PosMapa {
    let base = this as usize;
    // SAFETY: `base+0x50` es la posicion de la entidad (verificado sobre el
    // symtab y el codigo del motor: `scvtf` con desplazamiento 8 = /256).
    let pos = unsafe { std::ptr::read_unaligned((base + 0x50) as *const [i32; 2]) };
    PosMapa { x: pos[0], y: pos[1] }
}

/// Lee la direccion de la entidad (`Entity+0x280`, u8 con 4 bits utiles).
fn leer_direccion(this: *mut c_void) -> u8 {
    let base = this as usize;
    // SAFETY: `base+0x280` es el byte de direccion.
    (unsafe { std::ptr::read_unaligned((base + 0x280) as *const u8) }) & 0x0F_u8
}

/// Hook de `DrawQueue::draw`.
///
/// El motor pasa aqui cada sprite que va a dibujar. Para las entidades, la
/// `MapPosition` es la de la entidad, asi que comparandola con las que
/// hemos visto en `AssemblingMachine::draw` sabemos que sprite es el
/// nuestro.
///
/// Ademas, el `DrawQueue` guarda los `RenderParameters` en su primer
/// puntero, y ahi estan la camara y la escala con las que el motor
/// convierte las coordenadas del mapa en coordenadas del mundo (la misma
/// cuenta que hace `SpriteDrawOrder::setup`, desensamblada):
///
///     mundo = (mapa / 256) * (32 * escala) - camara
#[no_mangle]
pub extern "C" fn xfv_hook_draw_queue(
    this: *mut c_void,
    sprite: *mut c_void,
    position: *mut c_void,
    flags: u32,
    layer: u32,
    shift: *mut c_void,
    _size_x: f32,
    _size_y: f32,
) {
    hook_draw_queue(this, sprite, position, flags, layer, shift, _size_x, _size_y)
}

extern "C" fn hook_draw_queue(
    this: *mut c_void,
    sprite: *mut c_void,
    position: *mut c_void,
    flags: u32,
    layer: u32,
    shift: *mut c_void,
    _size_x: f32,
    _size_y: f32,
) {
    // El original primero: el sprite tiene que existir.
    let tramp = TRAMPOLIN_DQ.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookDrawQueue = std::mem::transmute(tramp);
            f(this, sprite, position, flags, layer, shift, _size_x, _size_y);
        }
    }

    if ACTIVO.load(Ordering::Relaxed) == 0 || position.is_null() || this.is_null() {
        return;
    }
    // Comprobacion rapida antes de tocar el estado compartido: este hook se
    // llama para cada sprite del mundo.
    if N_ENTIDADES.load(Ordering::Relaxed) == 0 {
        return;
    }

    // La posicion de mapa del sprite.
    // SAFETY: `position` apunta a un `MapPosition` (2 x i32).
    let mp = unsafe { std::ptr::read_unaligned(position as *const [i32; 2]) };
    let pos = PosMapa { x: mp[0], y: mp[1] };

    // ¿Es el sprite de una de nuestras entidades?
    let direccion = match con_estado(|e| e.direccion_de(pos)) {
        Some(d) => d,
        None => return,
    };

    // Los `RenderParameters` (el motor los guarda en el primer puntero del
    // `DrawQueue`).
    // SAFETY: `this` es el `DrawQueue`.
    let rp = unsafe { std::ptr::read_unaligned(this as *const *mut c_void) };
    if rp.is_null() {
        return;
    }
    // SAFETY: offsets verificados en el desensamblado de
    // `SpriteDrawOrder::setup`: la escala en +0x190 y la camara en +0x1b8.
    let (escala, cam_x, cam_y) = unsafe {
        let p = rp as usize;
        (
            std::ptr::read_unaligned((p + 0x190) as *const f64),
            std::ptr::read_unaligned((p + 0x1b8) as *const f64),
            std::ptr::read_unaligned((p + 0x1c0) as *const f64),
        )
    };

    // La posicion en el mundo del motor, con su misma cuenta.
    let factor = 32.0 * escala;
    let wx = (pos.x as f64 / 256.0) * factor - cam_x;
    let wy = (pos.y as f64 / 256.0) * factor - cam_y;

    // El tamano del sprite. El motor lo guarda en el propio sprite (dos
    // floats en +0x38) y lo pasa por las coordenadas del mundo
    // multiplicandolo por la escala (ver `SpriteDrawOrder::setup`).
    let (sx, sy) = if sprite.is_null() {
        (0.0, 0.0)
    } else {
        // SAFETY: `sprite` es un `Sprite`; +0x38 son sus dos floats de
        // tamano (verificado en el desensamblado de `setup`).
        unsafe {
            let p = sprite as *const f32;
            (
                std::ptr::read_unaligned(p.add(14)) as f64 * escala,
                std::ptr::read_unaligned(p.add(15)) as f64 * escala,
            )
        }
    };

    if sx <= 0.0 || sy <= 0.0 {
        return;
    }

    let caja = Caja {
        wx: wx as f32,
        wy: wy as f32,
        sx: sx as f32,
        sy: sy as f32,
        direccion,
    };
    con_estado(|e| e.anade_caja(caja));
    PENDIENTES.fetch_add(1, Ordering::Relaxed);

    let total = PENDIENTES.load(Ordering::Relaxed);
    if total <= 5 {
        log::info(&format!(
            "caja: mundo=({:.2}, {:.2}) tamano={:.1}x{:.1} dir={} escala={:.4} cam=({:.1}, {:.1})",
            caja.wx, caja.wy, caja.sx, caja.sy, caja.direccion, escala, cam_x, cam_y
        ));
    }
}

/// Hook de `DrawCommandBatch::flushSpritesBatch(bool)`.
///
/// Es el punto donde el motor emite sus ordenes GL de verdad. Justo despues
/// del flush, el mundo ya esta pintado en el framebuffer del mundo (que sigue
/// enlazado): ahi se pinta el cubo y se captura la imagen.
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
    pintar_si_toca();
}

/// Hook de `DrawCommandBatch::flushSpritesInstancedBatch(bool)`.
#[no_mangle]
pub extern "C" fn xfv_hook_flush_sprites_instanced(this: *mut c_void, flag: u8) {
    hook_flush_sprites_instanced(this, flag)
}

extern "C" fn hook_flush_sprites_instanced(this: *mut c_void, flag: u8) {
    let tramp = TRAMPOLIN_FLUSH_INST.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookFlush = std::mem::transmute(tramp);
            f(this, flag);
        }
    }
    pintar_si_toca();
}

/// Hook de `DrawCommandBatch::flushPrimitiveBatch()`.
#[no_mangle]
pub extern "C" fn xfv_hook_flush_primitives(this: *mut c_void) {
    hook_flush_primitives(this)
}

extern "C" fn hook_flush_primitives(this: *mut c_void) {
    let tramp = TRAMPOLIN_FLUSH_PRIM.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookFlushSimple = std::mem::transmute(tramp);
            f(this);
        }
    }
    pintar_si_toca();
}

/// Hook de `DrawCommandBatch::flushTextBatch()`.
#[no_mangle]
pub extern "C" fn xfv_hook_flush_text(this: *mut c_void) {
    hook_flush_text(this)
}

extern "C" fn hook_flush_text(this: *mut c_void) {
    let tramp = TRAMPOLIN_FLUSH_TEXT.load(Ordering::Relaxed);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookFlushSimple = std::mem::transmute(tramp);
            f(this);
        }
    }
    pintar_si_toca();
}

/// Pinta el cubo y hace la captura, si toca.
///
/// Solo se pinta en el framebuffer del mundo (el que no es el de la ventana):
/// ahi es donde el motor ha dibujado la escena y donde luego la compone.
fn pintar_si_toca() {
    if ACTIVO.load(Ordering::Relaxed) == 0 || USAR_GL.load(Ordering::Relaxed) == 0 {
        return;
    }
    // La primera vez se crea el programa y los buffers (ya hay contexto GL).
    preparar_gl();
    let gl = match GL.get() {
        Some(g) => g,
        None => return,
    };

    // El framebuffer de dibujado: el del mundo tiene id distinto de 0.
    let fb = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING);
    if fb == 0 {
        // Es la ventana (la interfaz): aqui no se pinta el cubo.
        return;
    }

    // Solo si el framebuffer es grande (el del mundo); los pequeños son
    // minimapas y similares.
    let mut vp = [0i32; 4];
    // SAFETY: el contexto GL esta activo.
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
    if vp[2] < 320 || vp[3] < 240 {
        return;
    }

    if DIAG_GL.swap(1, Ordering::Relaxed) == 0 {
        let muestras = gl_int(gl, GL_SAMPLES);
        log::info(&format!(
            "diagnostico GL: framebuffer={fb} viewport={}x{} muestras={muestras}",
            vp[2], vp[3]
        ));
    }

    dibujar_cajas(gl, &vp);
    capturar_si_pide(gl, &vp);
}

// --------------------------------------------------------------------------
// Dibujo del cubo
// --------------------------------------------------------------------------

/// Inclinacion del cubo respecto al suelo, en radianes (~31 grados).
///
/// Sin inclinacion, la proyeccion del motor (que solo tiene eje X e Y, es
/// ortografica) aplastaria el cubo a un rectangulo plano. Inclinandolo se ve
/// el volumen, como en la vista isometrica del juego.
const INCLINACION: f32 = 0.55;

/// Calcula la matriz del cubo (4x4 en column-major, como espera GL).
///
/// Transformacion (en el espacio del mundo del motor: x derecha, y abajo en
/// pantalla, z arriba):
///
///     M = T(wx, wy) * Rx(inclinacion) * T(0, 0, sz/2) * Rz(direccion) * S
///
///   * `Rz(direccion)` es la orientacion real de la entidad (16 pasos de
///     22,5 grados),
///   * `Rx(inclinacion)` inclina el cubo para que se vea su volumen con la
///     proyeccion ortografica del motor,
///   * `S` es el tamano real de la maquina (el de su sprite),
///   * la traslacion en Z deja la base del cubo en el suelo, en la posicion
///     de la entidad.
fn matriz_cubo(c: &Caja) -> [f32; 16] {
    // Rotacion de la entidad: los 16 pasos de direccion son 22,5 grados.
    let ang = (c.direccion as f32) * (core::f32::consts::TAU / 16.0);
    let (sy, cy) = (ang.sin(), ang.cos());
    // Inclinacion respecto al suelo.
    let (st, ct) = (INCLINACION.sin(), INCLINACION.cos());

    // Escala real de la maquina, en unidades del motor.
    let ex = c.sx;
    let ey = c.sy;
    let ez = c.sx;

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
        c.wx,
        c.wy - st * ez * 0.5,
        ct * ez * 0.5,
        1.0,
    ]
}

/// Calcula los vertices del cubo en 2D (la proyeccion 3D se hace aqui, en la
/// CPU) y devuelve el buffer de vertices (posicion + normal).
fn vertices_cubo(c: &Caja) -> [f32; 144] {
    let m = matriz_cubo(c);
    let mut out = [0f32; 144];
    for v in 0..24 {
        let px = ESQUINAS[v * 6];
        let py = ESQUINAS[v * 6 + 1];
        let pz = ESQUINAS[v * 6 + 2];
        let nx = ESQUINAS[v * 6 + 3];
        let ny = ESQUINAS[v * 6 + 4];
        let nz = ESQUINAS[v * 6 + 5];

        // Posicion transformada (column-major: x' = m0*x + m4*y + m8*z + m12).
        let x = m[0] * px + m[4] * py + m[8] * pz + m[12];
        let y = m[1] * px + m[5] * py + m[9] * pz + m[13];

        // Normal transformada (sin traslacion).
        let ax = m[0] * nx + m[4] * ny + m[8] * nz;
        let ay = m[1] * nx + m[5] * ny + m[9] * nz;
        let az = m[2] * nx + m[6] * ny + m[10] * nz;
        let largo = (ax * ax + ay * ay + az * az).sqrt().max(1e-6);

        out[v * 6] = x;
        out[v * 6 + 1] = y;
        out[v * 6 + 2] = 0.0;
        out[v * 6 + 3] = ax / largo;
        out[v * 6 + 4] = ay / largo;
        out[v * 6 + 5] = az / largo;
    }
    out
}

/// Ordena las caras del cubo de mas lejos a mas cerca de la camara.
///
/// El depth test se dibuja desactivado (para que el cubo se vea siempre por
/// encima de la escena), asi que el orden de dibujo es el que decide que cara
/// tapa a cual.
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

    // Direccion de la camara (en el espacio del juego: x derecha, y abajo,
    // z arriba). La camara esta por encima y al norte.
    const VISTA: [f32; 3] = [0.0, 0.70711, -0.70711];

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

    let mut out = [0u32; 36];
    for (k, &cara) in caras.iter().enumerate() {
        for j in 0..6 {
            out[k * 6 + j] = INDICES[cara * 6 + j];
        }
    }
    out
}

/// Dibuja las cajas pendientes en el framebuffer que hay enlazado.
fn dibujar_cajas(gl: &Gl, vp: &[i32; 4]) {
    let cajas = con_estado(|e| e.toma_cajas());
    if cajas.is_empty() {
        return;
    }
    let cubo = match CUBO.get() {
        Some(c) => c,
        None => return,
    };

    // Se guarda todo el estado que se va a tocar, para devolverlo al motor
    // tal y como estaba: el juego sigue despues de nuestro dibujo.
    let prog_previo = gl_int(gl, GL_CURRENT_PROGRAM);
    let vao_previo = gl_int(gl, GL_VERTEX_ARRAY_BINDING);
    let vbo_previo = gl_int(gl, GL_ARRAY_BUFFER_BINDING);
    let depth_previo = gl_int(gl, GL_DEPTH_TEST);
    let blend_previo = gl_int(gl, GL_BLEND);
    let cull_previo = gl_int(gl, GL_CULL_FACE);

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
    }
    drenar_errores(gl, "preparar el estado de dibujo");

    for c in cajas.iter() {
        if c.sx <= 0.0 || c.sy <= 0.0 {
            continue;
        }
        let vertices = vertices_cubo(&c);
        let m = matriz_cubo(&c);
        let indices = orden_caras(&m);

        unsafe {
            (gl.use_program)(cubo.programa);

            // El bloque `vsConstants` del motor (con su matriz de proyeccion)
            // se enlaza en nuestro programa en el mismo punto de binding.
            let (buffer_vs, binding_vs) = VS_DEL_JUEGO.load_or_store();
            if cubo.idx_vs != GL_INVALID_INDEX && buffer_vs != 0 {
                (gl.uniform_block_binding)(cubo.programa, cubo.idx_vs, binding_vs);
                (gl.bind_buffer_base)(GL_UNIFORM_BUFFER, binding_vs, buffer_vs);
            }

            (gl.bind_vertex_array)(cubo.vao);
            (gl.bind_buffer)(GL_ARRAY_BUFFER, cubo.vbo);
            (gl.buffer_data)(
                GL_ARRAY_BUFFER,
                (vertices.len() * 4) as isize,
                vertices.as_ptr() as *const c_void,
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
            drenar_errores(gl, "dibujar el cubo");
        }
    }

    // Se devuelve el estado al motor.
    unsafe {
        (gl.bind_vertex_array)(vao_previo as u32);
        (gl.bind_buffer)(GL_ARRAY_BUFFER, vbo_previo as u32);
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
    }
    if drenar_errores(gl, "restaurar el estado") {
        log::aviso("hubo errores de GL al dibujar el cubo");
    }
    let _ = vp;
}

// --------------------------------------------------------------------------
// Camara: el bloque `vsConstants` del motor
// --------------------------------------------------------------------------

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

/// Cache del indice del bloque `vsConstants` por programa GL.
///
/// Consultar el indice de un bloque de uniforms en cada llamada es caro
/// (el motor llama a `bindUniformBuffer` muchas veces por frame), asi que
/// se cachea: solo se vuelve a preguntar cuando cambia el programa.
static CACHE_PROGRAMA: AtomicU64 = AtomicU64::new(0);
static CACHE_INDICE: AtomicU64 = AtomicU64::new(u64::MAX);

/// Indice del bloque `vsConstants` de un programa (o `u64::MAX` si no lo
/// tiene).
fn indice_vs_constants(gl: &Gl, programa: u32) -> u32 {
    if CACHE_PROGRAMA.load(Ordering::Relaxed) == programa as u64 {
        return CACHE_INDICE.load(Ordering::Relaxed) as u32;
    }
    let nombre = b"vsConstants\0";
    let idx = unsafe { (gl.get_uniform_block_index)(programa, nombre.as_ptr() as *const i8) };
    CACHE_PROGRAMA.store(programa as u64, Ordering::Relaxed);
    CACHE_INDICE.store(idx as u64, Ordering::Relaxed);
    idx
}

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
    this: *mut c_void,
    index: u32,
    pipeline: *mut c_void,
    uniform_buffer: *mut c_void,
) {
    hook_bind_uniform_buffer(this, index, pipeline, uniform_buffer)
}

extern "C" fn hook_bind_uniform_buffer(
    this: *mut c_void,
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
    // Cinturon de seguridad: el indice es un puntero dentro del array de
    // bloques del pipeline state. Si algo no cuadra, no se lee memoria.
    if !uniform_buffer.is_null() && !pipeline.is_null() && index < 256 {
        // SAFETY: offsets verificados en el desensamblado del motor.
        let gl_buffer =
            unsafe { std::ptr::read_unaligned((uniform_buffer as usize + 0x10) as *const u32) };
        let programa =
            unsafe { std::ptr::read_unaligned((pipeline as usize + 0x20) as *const u32) };
        let tabla = unsafe { std::ptr::read_unaligned((pipeline as usize + 0x78) as *const usize) };
        if gl_buffer != 0 && programa != 0 && tabla != 0 {
            let bloque =
                unsafe { std::ptr::read_unaligned((tabla + index as usize * 4) as *const u32) };
            if DIAG_UB.swap(1, Ordering::Relaxed) == 0 {
                log::info(&format!(
                    "bindUniformBuffer: index={index} pso={pipeline:p} ub={uniform_buffer:p} \
                     buffer={gl_buffer} programa={programa} tabla=0x{tabla:x} bloque={bloque}"
                ));
            }
            if bloque != GL_INVALID_INDEX && bloque != 0xFFFFFFFF {
                if let Some(gl) = GL.get() {
                    let idx = indice_vs_constants(gl, programa);
                    if idx == bloque {
                        let antes = VS_DEL_JUEGO.buffer.load(Ordering::Relaxed);
                        VS_DEL_JUEGO.buffer.store(gl_buffer as u64, Ordering::Relaxed);
                        VS_DEL_JUEGO.binding.store(index as u64, Ordering::Relaxed);
                        if antes == 0 {
                            log::info(&format!(
                                "vsConstants del motor: buffer={gl_buffer} binding={index}"
                            ));
                        }
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
            f(this, index, pipeline, uniform_buffer);
        }
    }
}

// --------------------------------------------------------------------------
// Captura
// --------------------------------------------------------------------------

/// Captura el framebuffer actual si la variable de entorno lo pide.
///
/// Se escribe en `XFACTORVR_3D_CAPTURA` (una ruta). Se capturan como mucho
/// unos pocos frames: la captura cuesta y el objetivo es tener UNA imagen
/// para mirar.
fn capturar_si_pide(gl: &Gl, vp: &[i32; 4]) {
    if CAPTURAR.load(Ordering::Relaxed) == 0 {
        return;
    }
    // Se captura de tarde en tarde: leer el framebuffer es caro y el
    // objetivo es tener UNA imagen para mirar.
    let n = CAPTURAS.fetch_add(1, Ordering::Relaxed);
    if n % CADA_CUANTAS_CAPTURAS != 0 {
        return;
    }
    let ruta = match std::env::var("XFACTORVR_3D_CAPTURA") {
        Ok(v) if !v.is_empty() => v,
        _ => return,
    };
    // Se escribe siempre en el mismo fichero: asi contiene siempre el frame
    // mas reciente (no hace falta saber en que momento se capturo).
    if capturar_png(gl, vp, &ruta) {
        log::info(&format!("captura escrita en {}", ruta));
    }
}

/// Cada cuantas capturas se escribe una (el framebuffer se lee entero, es
/// caro).
const CADA_CUANTAS_CAPTURAS: u64 = 20;

/// Captura el framebuffer actual y lo guarda como PNG.
///
/// Se lee del framebuffer que hay enlazado (el del mundo), que en el momento
/// del flush ya tiene la imagen final del frame.
fn capturar_png(gl: &Gl, vp: &[i32; 4], ruta: &str) -> bool {
    let w = vp[2];
    let h = vp[3];
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
    if drenar_errores(gl, "leer el framebuffer") {
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

    // GL devuelve las filas de abajo arriba. El motor dibuja el mundo con la
    // Y invertida, asi que en su framebuffer la imagen sale ya derecha; si no
    // es el caso, se puede voltear con `XFACTORVR_3D_VOLTEAR=1`.
    if CAPTURAR_VOLTEADA.load(Ordering::Relaxed) == 1 {
        let mut flipped = vec![0u8; pix.len()];
        let stride = (w * 4) as usize;
        for y in 0..h as usize {
            let src = (h as usize - 1 - y) * stride;
            let dst = y * stride;
            flipped[dst..dst + stride].copy_from_slice(&pix[src..src + stride]);
        }
        pix = flipped;
    }
    escribir_png(ruta, w as u32, h as u32, &pix)
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

// --------------------------------------------------------------------------
// Instalacion
// --------------------------------------------------------------------------

/// Instala los hooks del modulo 3D.
///
/// Los simbolos se buscan por su nombre MANGLED exacto, que es como esta en el
/// symtab del Factorio 2.1.22 ARM64 (verificado).
pub fn instalar(simbolos: &resolver::Simbolos) {
    // Con `XFACTORVR_3D=0` el modulo entero se apaga: no se instala ningun
    // detour (sirve para bisecar problemas del motor).
    if ACTIVO.load(Ordering::Relaxed) == 0 {
        log::info("modulo 3D apagado (XFACTORVR_3D=0): no se instala ningun hook");
        return;
    }

    // La direccion de carga de esta libreria: sirve para traducir las
    // direcciones que salen en las trazas de crash del motor.
    log::info(&format!("base de la .so: 0x{:x}", base_de_la_so()));

    // --- hook de la entidad ---
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

    match detour::instalar(encontrado.direccion, hook_addr, None, encontrado.tamano) {
        Ok(d) => {
            TRAMPOLIN.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!(
                "detour instalado sobre AssemblingMachine::draw(): trampolin en 0x{:x}",
                d.trampolin
            ));
        }
        Err(e) => {
            log::error(&format!("no se pudo instalar el detour de draw: {e}"));
        }
    }

    // --- hook de la cola de dibujado (posicion y tamano reales) ---
    //
    // `DrawQueue::draw` es la funcion que el motor usa para encolar cada
    // sprite. Para las entidades, la `MapPosition` que recibe es la de la
    // entidad, asi que se puede saber que sprite es el nuestro. Ademas, en
    // ella estan la camara y la escala con las que el motor pasa de
    // coordenadas del mapa a coordenadas del mundo.
    const MANGLE_DQ: &str =
        "_ZN9DrawQueue4drawERK6SpriteRK11MapPosition12DrawingFlagsN11RenderLayer4EnumERK6Vectoraf";
    if HOOK_SETUP.load(Ordering::Relaxed) == 0 {
        log::aviso("detour de DrawQueue::draw NO instalado (bisectado)");
    } else if let Some(dq) = simbolos.resolver_exacto(MANGLE_DQ) {
        if let Some(a) = direccion_de_ejecucion_de("xfv_hook_draw_queue") {
            match detour::instalar(dq.direccion, a, None, dq.tamano) {
                Ok(d) => {
                    TRAMPOLIN_DQ.store(d.trampolin as usize, Ordering::SeqCst);
                    log::info("detour instalado sobre DrawQueue::draw()");
                }
                Err(e) => log::error(&format!("no se pudo instalar el detour de draw: {e}")),
            }
        }
    } else {
        log::aviso("no se encontro DrawQueue::draw()");
    }

    // --- hook del enlace de uniform buffers (la camara del motor) ---
    const MANGLE_UB: &str =
        "_ZN23GraphicsInterfaceOpenGL17bindUniformBufferEjRK19PipelineStateObjectPK13UniformBuffer";
    if let Some(ub) = simbolos.resolver_exacto(MANGLE_UB) {
        if let Some(a) = direccion_de_ejecucion_de("xfv_hook_bind_uniform_buffer") {
            match detour::instalar(ub.direccion, a, None, ub.tamano) {
                Ok(d) => {
                    TRAMPOLIN_UB.store(d.trampolin as usize, Ordering::SeqCst);
                    log::info("detour instalado sobre bindUniformBuffer()");
                }
                Err(e) => log::error(&format!("no se pudo instalar el detour de bindUniformBuffer: {e}")),
            }
        }
    } else {
        log::aviso("no se encontro bindUniformBuffer()");
    }

    // --- hooks de los flush (el punto de dibujado) ---
    if HOOK_FLUSH.load(Ordering::Relaxed) == 0 {
        log::aviso("detours de los flush NO instalados (bisectado)");
        return;
    }
    instalar_flush(
        simbolos,
        "_ZN16DrawCommandBatch17flushSpritesBatchEb",
        "xfv_hook_flush_sprites",
        &TRAMPOLIN_FLUSH,
    );
    instalar_flush(
        simbolos,
        "_ZN16DrawCommandBatch26flushSpritesInstancedBatchEb",
        "xfv_hook_flush_sprites_instanced",
        &TRAMPOLIN_FLUSH_INST,
    );
    instalar_flush(
        simbolos,
        "_ZN16DrawCommandBatch19flushPrimitiveBatchEv",
        "xfv_hook_flush_primitives",
        &TRAMPOLIN_FLUSH_PRIM,
    );
    instalar_flush(
        simbolos,
        "_ZN16DrawCommandBatch14flushTextBatchEv",
        "xfv_hook_flush_text",
        &TRAMPOLIN_FLUSH_TEXT,
    );
}

/// Instala un detour sobre una funcion de flush.
fn instalar_flush(
    simbolos: &resolver::Simbolos,
    mangled: &str,
    nombre_hook: &str,
    trampolin: &AtomicUsize,
) {
    let f = match simbolos.resolver_exacto(mangled) {
        Some(f) => f,
        None => {
            log::aviso(&format!("no se encontro {}", mangled));
            return;
        }
    };
    let a = match direccion_de_ejecucion_de(nombre_hook) {
        Some(a) => a,
        None => {
            log::aviso(&format!("dlsym no devolvio {}", nombre_hook));
            return;
        }
    };
    match detour::instalar(f.direccion, a, None, f.tamano) {
        Ok(d) => {
            trampolin.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!("detour instalado sobre {}", mangled));
        }
        Err(e) => log::error(&format!("no se pudo instalar el detour de {mangled}: {e}")),
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

/// Direccion de carga de esta libreria (para traducir direcciones del crash).
pub fn base_de_la_so() -> u64 {
    extern "C" {
        fn dladdr(addr: *const core::ffi::c_void, info: *mut core::ffi::c_void) -> i32;
    }
    // Estructura `Dl_info` de glibc: los dos primeros campos son los que
    // nos interesan (base y nombre).
    #[repr(C)]
    struct DlInfo {
        dli_fname: *const core::ffi::c_char,
        dli_fbase: *mut core::ffi::c_void,
        dli_sname: *const core::ffi::c_char,
        dli_saddr: *mut core::ffi::c_void,
    }
    let mut info = DlInfo {
        dli_fname: core::ptr::null(),
        dli_fbase: core::ptr::null_mut(),
        dli_sname: core::ptr::null(),
        dli_saddr: core::ptr::null_mut(),
    };
    let f = xfv_hook_bind_uniform_buffer as *const () as *const core::ffi::c_void;
    unsafe {
        if dladdr(f, &mut info as *mut DlInfo as *mut core::ffi::c_void) != 0 {
            return info.dli_fbase as u64;
        }
    }
    0
}

/// Prepara el estado de GL (crea el programa y los buffers del cubo).
///
/// Se llama una vez, desde el hook de flush, cuando ya hay contexto GL.
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
    if let Ok(v) = std::env::var("XFACTORVR_3D_VOLTEAR") {
        CAPTURAR_VOLTEADA.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    for (var, destino) in [
        ("XFACTORVR_3D_HOOK_SETUP", &HOOK_SETUP),
        ("XFACTORVR_3D_HOOK_FLUSH", &HOOK_FLUSH),
        ("XFACTORVR_3D_GL", &USAR_GL),
    ] {
        if let Ok(v) = std::env::var(var) {
            destino.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
        }
    }
}

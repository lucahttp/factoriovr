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
//!   1. `DrawQueue::draw(sprite, position, ...)` encola cada sprite. Para las
//!      entidades, la `MapPosition` que recibe es la de la entidad, asi que se
//!      puede saber que sprite es el nuestro (la entidad se identifica
//!      porque su `MapPosition` coincide con la que leimos en `Entity+0x50`).
//!   2. Ese sprite se dibuja mas tarde dentro de un framebuffer propio
//!      (el "framebuffer del mundo"), que despues se compone en la ventana.
//!
//! Del `DrawQueue` tambien salen la camara y la escala (los `RenderParameters`
//! estan en su primer puntero), con las que el motor pasa de coordenadas del
//! mapa a coordenadas del mundo (ver la seccion "Camara").
//!
//! Ojo: **no** se engancha `SpriteDrawOrder::setup` (que seria lo ideal: ahi
//! quedan la posicion y el tamano exactos). Su prologo no se puede reubicar y
//! el detour corrompe el motor: el juego se cae con SIGSEGV dentro de `cfree`.
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
//! El motor dibuja el mundo con una proyeccion **ortografica** que vive en
//! el bloque de uniforms `vsConstants` (`{ mat4 projection; }` en std140, segun
//! `data/core/graphics/shaders/sprite.vert`). Esa matriz **se calcula aqui**,
//! sin preguntar al motor:
//!
//!   * `SpriteDrawOrder::setup` (desensamblado del motor 2.1.22) deja claro
//!     como pasa el motor de coordenadas del mapa a coordenadas del mundo:
//!
//!       ```text
//!         mundo = (mapa / 256) * (32 * escala) - camara
//!       ```
//!
//!     (es exactamente la cuenta de `0x179cf6c` y `0x179cf74`). Ojo: `mundo`
//!     NO es una posicion relativa a la camara, son **pixeles del framebuffer
//!     del mundo** (la camara va en la resta de arriba). Comprobado creando
//!     una maquina en el juego: su `mundo` era `(856, 384)` y en la captura
//!     salia en esos mismos pixeles,
//!   * el framebuffer del mundo se dibuja con la Y hacia abajo (al componerlo
//!     en la ventana el motor lo voltea): al volcarlo con `glReadPixels` sale
//!     derecho, con la GUI abajo. Por eso la Y del clip crece hacia abajo.
//!
//! La proyeccion queda entonces:
//!
//!   ```text
//!     ndc_x = mundo_x * 2 / W - 1
//!     ndc_y = mundo_y * 2 / H - 1
//!   ```
//!
//! donde W y H son los del viewport del framebuffer del mundo.
//!
//! **Por que no se le pregunta al motor**: la version anterior enganchaba
//! `GraphicsInterfaceOpenGL::bindUniformBuffer` y llamaba a
//! `glGetUniformBlockIndex` sobre el *programa del motor* para averiguar que
//! bloque era `vsConstants`. Eso hacia que el juego petara con SIGSEGV: en
//! cuanto el motor cambiaba de programa se repetia la consulta y algo se
//! rompia. Ese hook ahora **no toca la GL en absoluto**: solo apunta que
//! buffers y que puntos de binding ve (ver `Enlace`), por si hace falta
//! diagnosear.
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
use std::sync::atomic::{AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::detour::{self};
use crate::log;
use crate::resolver;

/// Direccion del trampolin de `AssemblingMachine::draw`.

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

/// Si vale 1, no se busca ningun modelo 3D (solo el cubo de prueba).
static SIN_MODELOS: AtomicU64 = AtomicU64::new(0);

/// Si vale 1, se dibuja tambien el cubo de prueba (ademas de los modelos).
static CUBO_ACTIVO: AtomicU64 = AtomicU64::new(0);

/// Si vale 1, el sprite 2D de las entidades con modelo NO se encola (el
/// modelo 3D lo sustituye por completo).
static SIN_SPRITE: AtomicU64 = AtomicU64::new(0);

/// Cada cuantos flush se dibujan los modelos (1 = en todos).
///
/// El motor dibuja el mundo por trozos y cada trozo vacia la cola de sprites:
/// si los modelos se dibujan en todos los flush, se redibujan (caro).  Con un
/// valor mayor se dibujan solo de tarde en tarde (puede que el mundo se
/// dibuje encima de ellos).
static CADA_FLUSH: AtomicU64 = AtomicU64::new(1);

/// Contador de flush (para el throttle).
static CONTADOR_FLUSH: AtomicU64 = AtomicU64::new(0);

/// Maximo de modelos que se dibujan en un frame (el motor dibuja cientos de
/// entidades: pintarlas todas es una burrada).
const MAX_INSTANCIAS: usize = 48;

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

/// Desplazamiento de la camara del motor, en pixeles del framebuffer.
///
/// Es lo que hace el ESTEREO REAL: el motor dibuja el mundo desplazado a la
/// derecha en un frame y a la izquierda en el siguiente, y el visor muestra
/// cada vista en el ojo que le toca.  La imagen que dibuja el motor es de
/// verdad (con el 3D dentro), asi que el paralaje es el de verdad.
///
/// Cero = desactivado (lo de antes: el visor desplaza la UV).
static DESPLAZAMIENTO_CAMARA: AtomicU64 = AtomicU64::new(0);

/// Numero de frame del motor (lo lleva la cuenta el hook de render).
static FRAME: AtomicU64 = AtomicU64::new(0);

/// El ultimo framebuffer del mundo en el que se ha dibujado (lo que hay que
/// leer para tener el frame COMPLETO: el motor dibuja el mundo por trozos y
/// a mitad de frame todavia falta medio mapa).
static FB_MUNDO: AtomicU64 = AtomicU64::new(0);

/// La camara BASE (sin desplazar) del frame actual.
static CAMARA_BASE: Mutex<(u64, f64)> = Mutex::new((u64::MAX, 0.0));


/// Ya se ha hecho el diagnostico de `bindUniformBuffer` (una sola vez).
static DIAG_UB: AtomicU64 = AtomicU64::new(0);

/// Cuantas cajas se guardan como maximo.
const MAX_PENDIENTES: usize = 64;

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
pub struct Caja {
    /// Posicion en el espacio del mundo del motor (x, y).
    pub wx: f32,
    pub wy: f32,
    /// Tamano del sprite de la entidad (ancho y alto) en unidades del motor.
    pub sx: f32,
    pub sy: f32,
    /// Direccion de la entidad (0..15).
    pub direccion: u8,
    /// Indice del modelo 3D en el registro (-1 = sin modelo).
    pub modelo: i32,
    /// Pixels por casilla (el zoom del motor).
    pub escala: f32,
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
        modelo: -1,
        escala: 32.0,
    }; MAX_PENDIENTES],
    n_pend: 0,
    visibles: [Caja {
        wx: 0.0,
        wy: 0.0,
        sx: 0.0,
        sy: 0.0,
        direccion: 0,
        modelo: -1,
        escala: 32.0,
    }; MAX_PENDIENTES],
    n_visibles: 0,
    entidades: [Entidad {
        pos: PosMapa { x: 0, y: 0 },
        direccion: 0,
        modelo: -1,
    }; MAX_ENTIDADES],
    n_entidades: 0,
});

/// Estado compartido entre los hooks.
struct Estado {
    /// Cajas pendientes: las ha llenado el hook de `DrawQueue::draw` con las
    /// entidades que se han dibujado en este frame.
    pendientes: [Caja; MAX_PENDIENTES],
    /// Numero de cajas pendientes.
    n_pend: usize,
    /// Cajas del frame anterior: las que se dibujan en cada flush.
    ///
    /// Se guardan aparte de las pendientes porque el cubo tiene que estar en
    /// el framebuffer **siempre** (la captura se hace cada N flushes, y solo
    /// con el cubo ahi se puede comprobar que se ve).
    visibles: [Caja; MAX_PENDIENTES],
    /// Numero de cajas visibles.
    n_visibles: usize,
    /// Entidades vistas en el frame.
    entidades: [Entidad; MAX_ENTIDADES],
    /// Numero de entidades vistas.
    n_entidades: usize,
}

impl Estado {
    /// Anota una entidad (la llama el hook de `AssemblingMachine::draw`).
    fn anota_entidad(&mut self, pos: PosMapa, direccion: u8, modelo: i32) {
        let mut i = self.n_entidades;
        if i >= MAX_ENTIDADES {
            i = 0;
        }
        self.entidades[i] = Entidad {
            pos,
            direccion,
            modelo,
        };
        self.n_entidades = i + 1;
    }

    /// ¿Esta posicion de mapa es la de alguna entidad vista?
    ///
    /// Devuelve su direccion y su modelo.
    fn entidad_de(&self, pos: PosMapa) -> Option<(u8, i32)> {
        let mut i = 0usize;
        while i < self.n_entidades && i < MAX_ENTIDADES {
            if self.entidades[i].pos == pos {
                return Some((self.entidades[i].direccion, self.entidades[i].modelo));
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

    /// Publica las cajas de este frame y devuelve las que hay que dibujar.
    ///
    /// Las cajas del frame actual pasan a ser las "visibles" (las que se
    /// dibujan en cada flush, para que el cubo este siempre en el
    /// framebuffer). Las pendientes se vacian: si no llegara ninguna caja
    /// nueva, las visibles se quedan como estaban.
    fn publica_cajas(&mut self) -> Vec<Caja> {
        if self.n_pend > 0 {
            let n = self.n_pend.min(MAX_PENDIENTES);
            let mut i = 0usize;
            while i < n {
                self.visibles[i] = self.pendientes[i];
                i += 1;
            }
            self.n_visibles = n;
            self.n_pend = 0;
        }
        let n = self.n_visibles.min(MAX_PENDIENTES);
        let mut out = Vec::with_capacity(n);
        let mut i = 0usize;
        while i < n {
            out.push(self.visibles[i]);
            i += 1;
        }
        out
    }
}

/// Las entidades visibles del frame actual.
///
/// Es lo que viaja al visor por el canal de geometria (ADR-021): la
/// posicion, el tamano, la direccion y el modelo 3D de cada entidad, tal
/// cual las dibuja el motor (no estimadas como las del snapshot).
pub fn entidades_visibles() -> Vec<Caja> {
    con_estado(|e| {
        let n = e.n_visibles.min(MAX_PENDIENTES);
        let mut out = Vec::with_capacity(n);
        let mut i = 0usize;
        while i < n {
            out.push(e.visibles[i]);
            i += 1;
        }
        out
    })
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
    /// Indice del modelo 3D en el registro (-1 = sin modelo).
    modelo: i32,
}

/// Cuantas entidades se guardan como maximo.
const MAX_ENTIDADES: usize = 64;

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
/// `glScissor(x, y, width, height)`: fija la caja de recorte.
type GlScissor = unsafe extern "C" fn(x: i32, y: i32, width: i32, height: i32);
/// `glBindFramebuffer(target, framebuffer)`.
type GlBindFramebuffer = unsafe extern "C" fn(target: u32, framebuffer: u32);
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
/// `glCheckFramebufferStatus`: dice si un framebuffer esta COMPLETO (se puede
/// leer de el) o por que no.
type GlCheckFramebufferStatus = unsafe extern "C" fn(target: u32) -> u32;

// Codigos de GL usados.
const GL_NO_ERROR: u32 = 0;
const GL_CURRENT_PROGRAM: u32 = 0x8B8D;
const GL_COMPILE_STATUS: u32 = 0x8B81;
const GL_LINK_STATUS: u32 = 0x8B82;
const GL_VERTEX_SHADER: u32 = 0x8B31;
const GL_FRAGMENT_SHADER: u32 = 0x8B30;
const GL_ARRAY_BUFFER: u32 = 0x8892;
const GL_ELEMENT_ARRAY_BUFFER: u32 = 0x8893;
const GL_STATIC_DRAW: u32 = 0x88E4;
const GL_TRIANGLES: u32 = 0x0004;
const GL_FLOAT: u32 = 0x1406;
const GL_UNSIGNED_INT: u32 = 0x1405;
const GL_DEPTH_TEST: u32 = 0x0B71;
const GL_SCISSOR_BOX: u32 = 0x0C10;
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
const GL_PACK_ALIGNMENT: u32 = 0x0D05;
/// Framebuffer de dibujado actualmente enlazado.
const GL_DRAW_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
const GL_READ_FRAMEBUFFER_BINDING: u32 = 0x8CAA;
/// Objetivo de `glBindFramebuffer` para elegir solo el framebuffer de lectura.
const GL_READ_FRAMEBUFFER: u32 = 0x8CA8;
/// Objetivo de `glBindFramebuffer` para elegir solo el framebuffer de dibujo.
const GL_DRAW_FRAMEBUFFER: u32 = 0x8CA9;
/// Vertex array actualmente enlazado.
const GL_VERTEX_ARRAY_BINDING: u32 = 0x85B5;
/// Buffer de vertices actualmente enlazado.
const GL_ARRAY_BUFFER_BINDING: u32 = 0x8894;
/// Numero de muestras del framebuffer (multisampling).
const GL_SAMPLES: u32 = 0x80A9;
/// El framebuffer esta completo: se puede leer de el.
const GL_FRAMEBUFFER_COMPLETE: u32 = 0x8CD5;

/// Punteros a las funciones GL del motor.
///
/// El motor carga el GL con GLAD y guarda los punteros en variables globales
/// (`glad_glCreateShader` y compañía). Como GLAD las carga **despues** de que
/// arranque el inyector, al instalar solo se puede resolver la *direccion* de
/// esas variables; el puntero se lee en el primer frame con contexto GL.
struct Gl {
    get_integerv: GlGetIntegerv,
    get_error: GlGetError,
    scissor: GlScissor,
    bind_framebuffer: GlBindFramebuffer,
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
    disable: GlDisable,
    enable: GlEnable,
    blend_func_separate: GlBlendFuncSeparate,
    read_pixels: GlReadPixels,
    pixel_storei: GlPixelStorei,
    finish: GlFinish,
    get_string: GlGetString,
    check_framebuffer_status: Option<GlCheckFramebufferStatus>,
}

/// Direcciones de las variables globales del motor que guardan los punteros a
/// las funciones GL. Se resuelven al instalar (el symtab esta disponible);
/// los punteros se leen despues, cuando GLAD ya los ha cargado.
struct GlAddr {
    get_integerv: usize,
    get_error: usize,
    scissor: usize,
    bind_framebuffer: usize,
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
}

/// Punteros a las funciones GL del motor (se rellenan al instalar).
static GL: OnceLock<Gl> = OnceLock::new();

/// Direcciones de las variables globales con los punteros a las funciones GL.
static GL_DIR: OnceLock<GlAddr> = OnceLock::new();

/// Programa y buffers del cubo (se crean en el primer frame con contexto GL).
static CUBO: OnceLock<Cubo> = OnceLock::new();

/// Shader del cubo.
///
/// El cubo se calcula entero en la CPU: la transformacion 3D (giro,
/// inclinacion y escala) **y** la proyeccion de la camara. Al vertex shader
/// solo le llega el punto ya proyectado a coordenadas de clip, asi que no
/// hace falta ni bloque de uniforms ni matriz del motor.
///
/// La proyeccion se aplica al final: `ndc = mundo * 2 / viewport`.
const VS: &[u8] = b"#version 330\n\
layout(location = 0) in vec3 position;\n\
layout(location = 1) in vec3 normal;\n\
out vec3 vNormal;\n\
void main() {\n\
    gl_Position = vec4(position.xy, 0.0, 1.0);\n\
    vNormal = normal;\n\
}\n";

const FS: &[u8] = b"#version 330\n\
in vec3 vNormal;\n\
out vec4 fragColor;\n\
void main() {\n\
    vec3 n = normalize(vNormal);\n\
    vec3 luz = normalize(vec3(0.4, 0.6, 0.7));\n\
    float d = max(dot(n, luz), 0.0);\n\
    vec3 color = vec3(0.90, 0.10, 0.75) * (0.35 + 0.75 * d);\n\
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
        scissor: dir!("glad_glScissor"),
        bind_framebuffer: dir!("glad_glBindFramebuffer"),
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
        disable: dir!("glad_glDisable"),
        enable: dir!("glad_glEnable"),
        blend_func_separate: dir!("glad_glBlendFuncSeparate"),
        read_pixels: dir!("glad_glReadPixels"),
        pixel_storei: dir!("glad_glPixelStorei"),
        finish: dir!("glad_glFinish"),
        get_string: dir!("glad_glGetString"),
    })
}

/// Direccion de `SDL_GL_GetProcAddress` del motor (para cargar extensiones GL en runtime).
static SDL_GET_PROC_ADDRESS: AtomicUsize = AtomicUsize::new(0);

/// Resuelve un simbolo de OpenGL en tiempo de ejecucion usando SDL_GL_GetProcAddress
/// o dlsym.
pub fn resolver_proc_gl(nombre: &str) -> *mut c_void {
    let mut c_nombre = Vec::with_capacity(nombre.len() + 1);
    c_nombre.extend_from_slice(nombre.as_bytes());
    c_nombre.push(0);

    let get_proc = SDL_GET_PROC_ADDRESS.load(Ordering::Relaxed);
    if get_proc != 0 {
        let f: unsafe extern "C" fn(*const std::os::raw::c_char) -> *mut c_void =
            unsafe { std::mem::transmute(get_proc) };
        let ptr = unsafe { f(c_nombre.as_ptr() as *const std::os::raw::c_char) };
        if !ptr.is_null() {
            return ptr;
        }
    }
    unsafe {
        extern "C" {
            fn dlsym(handle: *mut c_void, symbol: *const std::os::raw::c_char) -> *mut c_void;
        }
        dlsym(std::ptr::null_mut(), c_nombre.as_ptr() as *const std::os::raw::c_char)
    }
}

/// Consulta si el framebuffer enlazado a `target` esta completo para lectura/dibujo.
fn check_fb_status(gl: &Gl, target: u32) -> u32 {
    if let Some(f) = gl.check_framebuffer_status {
        unsafe { f(target) }
    } else {
        GL_FRAMEBUFFER_COMPLETE
    }
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

    let check_framebuffer_status: Option<GlCheckFramebufferStatus> = {
        let ptr = resolver_proc_gl("glCheckFramebufferStatus");
        if !ptr.is_null() {
            Some(unsafe { core::mem::transmute(ptr) })
        } else {
            None
        }
    };

    Some(Gl {
        get_integerv: ptr!(get_integerv),
        get_error: ptr!(get_error),
        scissor: ptr!(scissor),
        bind_framebuffer: ptr!(bind_framebuffer),
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
        disable: ptr!(disable),
        enable: ptr!(enable),
        blend_func_separate: ptr!(blend_func_separate),
        read_pixels: ptr!(read_pixels),
        pixel_storei: ptr!(pixel_storei),
        finish: ptr!(finish),
        get_string: ptr!(get_string),
        check_framebuffer_status,
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

    Some(Cubo {
        programa: prog,
        vao,
        vbo,
        ebo,
    })
}

// --------------------------------------------------------------------------
// Hooks
// --------------------------------------------------------------------------

/// Tipo del trampolin de `DrawQueue::draw`.
///
///   ```cpp
///     draw(DrawQueue* this, Sprite const& sprite, MapPosition const& position,
///          DrawingFlags flags, RenderLayer::Enum layer, Vector const& shift,
///          float size_x, float size_y)
///   ```
///
/// Los dos `float` van en los registros s0 y s1 (AAPCS64: los argumentos
/// en coma flotante tienen sus propios registros).
type HookDrawQueue =
    unsafe extern "C" fn(*mut c_void, *mut c_void, *mut c_void, u32, u32, *mut c_void, f32, f32);

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
///   ```cpp
///     bindUniformBuffer(GraphicsInterfaceOpenGL* this, uint index,
///                       PipelineStateObject const* pso, UniformBuffer const* ub)
///   ```
type HookUniformBuffer = unsafe extern "C" fn(*mut c_void, u32, *mut c_void, *mut c_void);

/// Tipo del trampolin de `flushSpritesBatch`.
///
/// `void flushSpritesBatch(bool)`.
type HookFlush = unsafe extern "C" fn(*mut c_void, u8);

/// Tipo del trampolin de `flushPrimitiveBatch` / `flushTextBatch` (sin args).
type HookFlushSimple = unsafe extern "C" fn(*mut c_void);

/// Trampolin de `bindUniformBuffer`.
static TRAMPOLIN_UB: AtomicUsize = AtomicUsize::new(0);


/// Tipos de entidad a los que se engancha el 3D.
///
/// Todos tienen `draw(DrawQueue&)` (la misma firma que la de la maquina de
/// montar), asi que el hook es el mismo: lo que cambia es el simbolo donde
/// se engancha y el trampolin que hay que llamar.
///
/// Se han elegido las estructuras que definen una fabrica.  Los objetos con
/// MUCHOS sprites (cintas, insertores, tuberias) NO se enganchan: el hook
/// averigua el tamano del sprite por la posicion, y con varias clases de
/// sprite por entidad el tamano sale mal (cajas gigantes).
const TIPOS_3D: &[(&str, &str)] = &[
    ("_ZNK17AssemblingMachine4drawER9DrawQueue", "AssemblingMachine"),
    ("_ZNK7Furnace4drawER9DrawQueue", "Furnace"),
    ("_ZNK11MiningDrill4drawER9DrawQueue", "MiningDrill"),
    ("_ZNK17LogisticContainer4drawER9DrawQueue", "LogisticContainer"),
    ("_ZNK11Accumulator4drawER9DrawQueue", "Accumulator"),
    ("_ZNK11StorageTank4drawER9DrawQueue", "StorageTank"),
    ("_ZNK12CargoStation4drawER9DrawQueue", "CargoStation"),
    ("_ZNK10RocketSilo4drawER9DrawQueue", "RocketSilo"),
    ("_ZNK3Lab4drawER9DrawQueue", "Lab"),
    ("_ZNK6Boiler4drawER9DrawQueue", "Boiler"),
    ("_ZNK12ElectricPole4drawER9DrawQueue", "ElectricPole"),
    ("_ZNK8Roboport4drawER9DrawQueue", "Roboport"),
    ("_ZNK5Radar4drawER9DrawQueue", "Radar"),
    ("_ZNK8Inserter4drawER9DrawQueue", "Inserter"),
    ("_ZNK6Turret4drawER9DrawQueue", "Turret"),
    ("_ZNK10AmmoTurret4drawER9DrawQueue", "AmmoTurret"),
    ("_ZNK15ContainerEntity4drawER9DrawQueue", "ContainerEntity"),
    ("_ZNK15LinkedContainer4drawER9DrawQueue", "LinkedContainer"),
];

/// Maximo de tipos de entidad con 3D (tamano de los arrays de trampolines).
const MAX_TIPOS_3D: usize = 24;

/// Trampolin de `DrawQueue::draw`.
static TRAMPOLIN_DQ: AtomicUsize = AtomicUsize::new(0);

// --- hook del blit del framebuffer (el GUI del juego) ------------------------
//
// El motor dibuja el MUNDO en su framebuffer y compone el GUI (hotbar,
// minimapa, alertas) en OTRO, que despues se blitea a la pantalla.  Medido:
// el frame que llega al visor (el del mundo) NO trae el GUI, y el
// framebuffer 0 (la pantalla) con el driver offscreen esta vacio.
//
// Asi que el GUI hay que leerlo de su propio framebuffer: justo antes del
// blit, ese framebuffer es el que esta activo.
static TRAMPOLIN_BLIT: AtomicUsize = AtomicUsize::new(0);

// --- hook de la captura de pantalla del motor (donde SI esta el GUI) ------
//
// `RenderUtil::takeScreenshots()` es la funcion que hace el motor cuando
// hay que guardar una captura: en ese momento renderiza el frame FINAL (con
// el GUI del juego) y lo lee.  Enganchandola se ve que framebuffer usa:
// ese es el del GUI.
static TRAMPOLIN_SCREEN: AtomicUsize = AtomicUsize::new(0);

/// `RenderUtil::takeScreenshots(const std::vector<ScreenshotRequest>&, Map&,
/// bool)`: es una funcion ESTATICA (sin `this`).
type HookTakeScreenshots = unsafe extern "C" fn(*const c_void, *mut c_void, u8);

#[no_mangle]
pub extern "C" fn xfv_hook_take_screenshots(
    peticiones: *const c_void,
    mapa: *mut c_void,
    mostrar: u8,
) {
    // El framebuffer que usa el motor para la captura: si ahi esta el GUI,
    // este es el framebuffer que hay que leer para Mandarlo al visor.
    if let Some(gl) = GL.get() {
        let mut vp = [0i32; 4];
        unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
        let fb = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING);
        log::info(&format!(
            "captura del motor: viewport {}x{} framebuffer {}",
            vp[2], vp[3], fb
        ));
        if std::env::var("XFACTORVR_CAPTURA_GUI").ok().as_deref() == Some("1") {
            if let Ok(ruta) = std::env::var("XFACTORVR_CAPTURA_GUI_RUTA") {
                if !ruta.is_empty() {
                    let ruta = format!("{ruta}.motor");
                    if capturar_png(gl, &vp, &ruta) {
                        log::info(&format!("captura del motor escrita en {ruta}"));
                    }
                }
            }
        }
    }
    let tramp = TRAMPOLIN_SCREEN.load(Ordering::SeqCst);
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookTakeScreenshots = std::mem::transmute(tramp);
            f(peticiones, mapa, mostrar);
        }
    }
}

/// La firma de `FramebufferOpenGL::blitTo(VideoBitmap&, int, int, int, int,
/// int, int)`.
type HookBlitTo =
    unsafe extern "C" fn(*mut c_void, *mut c_void, i32, i32, i32, i32, i32, i32);

/// Framebuffers de destino y origen del ultimo blit del motor (donde se compone el GUI).
static ULTIMO_FB_BLIT_DESTINO: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static ULTIMO_FB_BLIT_ORIGEN: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

#[no_mangle]
pub extern "C" fn xfv_hook_blit_to(
    this: *mut c_void,
    destino: *mut c_void,
    a: i32,
    b: i32,
    c: i32,
    d: i32,
    e: i32,
    f: i32,
) {
    if let Some(gl) = GL.get() {
        let fb_dib = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING) as u32;
        let fb_lec = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING) as u32;
        if fb_dib != 0 {
            ULTIMO_FB_BLIT_DESTINO.store(fb_dib, Ordering::Relaxed);
        }
        if fb_lec != 0 {
            ULTIMO_FB_BLIT_ORIGEN.store(fb_lec, Ordering::Relaxed);
        }
    }
    // El framebuffer ACTIVO en este momento es el que se va a blitear.
    capturar_gui_si_pide();
    let tramp = TRAMPOLIN_BLIT.load(Ordering::SeqCst);
    if tramp == 0 {
        return;
    }
    // SAFETY: el trampolin contiene el bloque original reubicado y salta al
    // resto de la funcion; los argumentos se pasan intactos.
    unsafe {
        let g: HookBlitTo = std::mem::transmute(tramp);
        g(this, destino, a, b, c, d, e, f);
    }
}

/// Lee el framebuffer activo (el del GUI) y lo deja en un PNG.
///
/// Sirve para MIRAR el GUI del juego: con XFACTORVR_CAPTURA_GUI=1 y
/// XFACTORVR_CAPTURA_GUI_RUTA=/ruta.png.
fn capturar_gui_si_pide() {
    if std::env::var("XFACTORVR_CAPTURA_GUI").ok().as_deref() != Some("1") {
        return;
    }
    let Some(gl) = GL.get() else { return };
    let mut vp = [0i32; 4];
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
    let (w, h) = (vp[2], vp[3]);
    if w <= 0 || h <= 0 || w > 4096 || h > 4096 {
        return;
    }
    let mut pix = vec![0u8; (w as usize) * (h as usize) * 4];
    leer_pixeles(gl, w, h, &mut pix);
    let con_color = pix
        .chunks_exact(4)
        .filter(|p| p[0] != 0 || p[1] != 0 || p[2] != 0)
        .count();
    log::info(&format!(
        "GUI: framebuffer activo {w}x{h} con {} pixeles con color",
        con_color
    ));
    if let Ok(ruta) = std::env::var("XFACTORVR_CAPTURA_GUI_RUTA") {
        if !ruta.is_empty() {
            if capturar_png(gl, &vp, &ruta) {
                log::info(&format!("captura del GUI escrita en {ruta}"));
            }
        }
    }
}

/// Trampolines de los hooks de entidad (uno por tipo): cada tipo enganchado
/// necesita el suyo, porque el original es distinto en cada caso.
static TRAMPOLIN_ENT: [AtomicUsize; MAX_TIPOS_3D] = [const { AtomicUsize::new(0) }; MAX_TIPOS_3D];

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
    hook_draw_asm(this, queue, 0)
}

/// Un hook por tipo de entidad: cada uno llama a su trampolin.
macro_rules! hook_de_entidad {
    ($nombre:ident, $indice:expr) => {
        #[no_mangle]
        pub extern "C" fn $nombre(this: *mut c_void, queue: *mut c_void) {
            hook_draw_asm(this, queue, $indice)
        }
    };
}

hook_de_entidad!(xfv_hook_ent_0, 0);
hook_de_entidad!(xfv_hook_ent_1, 1);
hook_de_entidad!(xfv_hook_ent_2, 2);
hook_de_entidad!(xfv_hook_ent_3, 3);
hook_de_entidad!(xfv_hook_ent_4, 4);
hook_de_entidad!(xfv_hook_ent_5, 5);
hook_de_entidad!(xfv_hook_ent_6, 6);
hook_de_entidad!(xfv_hook_ent_7, 7);
hook_de_entidad!(xfv_hook_ent_8, 8);
hook_de_entidad!(xfv_hook_ent_9, 9);
hook_de_entidad!(xfv_hook_ent_10, 10);
hook_de_entidad!(xfv_hook_ent_11, 11);
hook_de_entidad!(xfv_hook_ent_12, 12);
hook_de_entidad!(xfv_hook_ent_13, 13);
hook_de_entidad!(xfv_hook_ent_14, 14);
hook_de_entidad!(xfv_hook_ent_15, 15);
hook_de_entidad!(xfv_hook_ent_16, 16);
hook_de_entidad!(xfv_hook_ent_17, 17);

/// Los hooks por tipo, en el mismo orden que `TIPOS_3D`.
static HOOKS_ENT: [unsafe extern "C" fn(*mut c_void, *mut c_void); 18] = [
    xfv_hook_ent_0,
    xfv_hook_ent_1,
    xfv_hook_ent_2,
    xfv_hook_ent_3,
    xfv_hook_ent_4,
    xfv_hook_ent_5,
    xfv_hook_ent_6,
    xfv_hook_ent_7,
    xfv_hook_ent_8,
    xfv_hook_ent_9,
    xfv_hook_ent_10,
    xfv_hook_ent_11,
    xfv_hook_ent_12,
    xfv_hook_ent_13,
    xfv_hook_ent_14,
    xfv_hook_ent_15,
    xfv_hook_ent_16,
    xfv_hook_ent_17,
];

extern "C" fn hook_draw_asm(this: *mut c_void, queue: *mut c_void, indice: usize) {
    let n = LLAMADAS.fetch_add(1, Ordering::Relaxed) + 1;

    // Guardar la posicion de mapa de la entidad: con ella se identifica su
    // sprite en el hook de `SpriteDrawOrder::setup`.
    if ACTIVO.load(Ordering::Relaxed) == 1 && !this.is_null() {
        let pos = leer_posicion(this);
        let direccion = leer_direccion(this);
        // El modelo 3D que le toca: se saca del PROTOTIPO de la entidad
        // ("assembling-machine-1", "stone-furnace"...).  El prototipo se
        // localiza en caliente (en esta build no hay `Entity::getPrototype`).
        let modelo = modelo_de_entidad(this, indice);
        con_estado(|e| e.anota_entidad(pos, direccion, modelo));
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
    let tramp = if indice < MAX_TIPOS_3D {
        TRAMPOLIN_ENT[indice].load(Ordering::Relaxed)
    } else {
        0
    };
    if tramp != 0 {
        // SAFETY: el trampolin repone los argumentos intactos.
        unsafe {
            let f: HookDraw = std::mem::transmute(tramp);
            f(this, queue);
        }
    }
}

/// El modelo 3D que le toca a una entidad (-1 = ninguno).
///
/// Se averigua por el PROTOTIPO de la entidad (el nombre "assembling-machine-1"
/// y compañía).  En esta build del motor no existe `Entity::getPrototype()`,
/// asi que el modulo `proto` localiza el puntero al prototipo en caliente.
fn modelo_de_entidad(this: *mut c_void, clase: usize) -> i32 {
    if SIN_MODELOS.load(Ordering::Relaxed) == 1 {
        return -1;
    }
    match crate::proto::nombre_de_prototipo(this, clase) {
        Some(nombre) => match crate::render3d::indice_de_prototipo(&nombre) {
            Some(i) => i as i32,
            None => -1,
        },
        None => -1,
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
///   ```text
///     mundo = (mapa / 256) * (32 * escala) - camara
///   ```

// --------------------------------------------------------------------------
// Estereo real: desplazar la camara del motor por ojo
// --------------------------------------------------------------------------

/// El desplazamiento configurado (en pixeles). Cero = desactivado.
pub fn desplazamiento_camara() -> f64 {
    f64::from_bits(DESPLAZAMIENTO_CAMARA.load(Ordering::Relaxed))
}

/// Avisa de que empieza un frame nuevo (lo llama el hook de render).
///
/// El desplazamiento de camara alterna: un frame a la derecha, el siguiente a
/// la izquierda.  Asi el visor puede enseñar cada vista en el ojo que le
/// corresponde y el paralaje es REAL (lo ha dibujado el motor).
pub fn nuevo_frame() {
    FRAME.fetch_add(1, Ordering::Relaxed);
}

/// El ojo al que pertenece el frame actual: 0 = izquierdo, 1 = derecho.
pub fn ojo_actual() -> u8 {
    (FRAME.load(Ordering::Relaxed) & 1) as u8
}

/// Desplaza la camara del motor ANTES de que dibuje.
///
/// La camara vive en los `RenderParameters` que el motor pasa al `DrawQueue`
/// (offset +0x1b8).  El motor calcula la posicion de cada sprite con ella
/// (`mundo = (mapa / 256) * (32 * escala) - camara`), asi que escribiendola
/// aqui ANTES del original, todo el frame sale desplazado lo mismo.
///
/// Se escribe un valor ABSOLUTO (la camara base + el desplazamiento), no un
/// "sumar": si no, la camara se iria acumulando frame a frame.
fn aplicar_desplazamiento_camara(this: *mut c_void) {
    let d = desplazamiento_camara();
    if d == 0.0 || this.is_null() {
        return;
    }
    // El primer puntero del DrawQueue son los RenderParameters.
    // SAFETY: `this` es el `DrawQueue` (ver el hook de abajo).
    let rp = unsafe { std::ptr::read_unaligned(this as *const *mut c_void) };
    if rp.is_null() {
        return;
    }
    let p = rp as usize;
    let frame = FRAME.load(Ordering::Relaxed);

    // La camara base: la que tiene el motor al empezar el frame.
    let base = {
        let mut g = CAMARA_BASE.lock().unwrap_or_else(|p| p.into_inner());
        if g.0 != frame {
            // SAFETY: offset verificado en el desensamblado de
            // `SpriteDrawOrder::setup` (la camara esta en +0x1b8).
            let actual = unsafe { std::ptr::read_unaligned((p + 0x1b8) as *const f64) };
            *g = (frame, actual);
        }
        g.1
    };

    // El desplazamiento alterna de signo cada frame.
    let signo = if ojo_actual() == 0 { 1.0 } else { -1.0 };
    let desplazada = base + signo * d;
    // SAFETY: offset verificado (ver arriba).
    unsafe { std::ptr::write_unaligned((p + 0x1b8) as *mut f64, desplazada) };
}

// ---------------------------------------------------------------------------
// Diagnostico: que CAPAS dibuja el motor (para el GUI como textura)
// ---------------------------------------------------------------------------
//
// El motor dibuja el mundo en una capa y el GUI (hotbar, minimapa, alertas)
// en otra.  El inyector necesita saber cual es la del GUI para poder leer el
// framebuffer ANTES de que se dibuja (el mundo sin GUI) y despues (con el
// GUI), y restar: GUI = F - W (ADR-022).
//
// Con XFACTORVR_DIAG_CAPAS=1 se vuelca la secuencia de capas de cada frame.

/// La ultima capa que se ha visto (para detectar los cambios).
static CAPA_ACTUAL: AtomicU32 = AtomicU32::new(u32::MAX);
/// Las capas de este frame: (capa, draws, x_min, y_min, x_max, y_max).
///
/// El rectangulo es el de las POSICIONES de los draws.  En el mundo son
/// coordenadas de tile (grandes); en el GUI son pixeles de pantalla
/// (0..ancho, 0..alto).  Eso es lo que dice que capa es el GUI.
#[derive(Clone, Copy)]
struct InfoCapa {
    draws: u32,
    x_min: i32,
    y_min: i32,
    x_max: i32,
    y_max: i32,
}

/// Las capas de este frame.
static CAPAS_FRAME: Mutex<Vec<(u32, InfoCapa)>> = Mutex::new(Vec::new());

/// Suma un draw a la capa actual y registra el cambio.
fn anotar_capa(layer: u32, x: i32, y: i32) {
    if std::env::var("XFACTORVR_DIAG_CAPAS").ok().as_deref() != Some("1") {
        return;
    }
    let anterior = CAPA_ACTUAL.swap(layer, Ordering::Relaxed);
    let mut guard = match CAPAS_FRAME.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    // Busca la entrada de esta capa (puede no ser la ultima: las capas
    // alternan).
    let pos = match guard.iter().position(|(c, _)| *c == layer) {
        Some(i) => i,
        None => {
            guard.push((
                layer,
                InfoCapa { draws: 0, x_min: i32::MAX, y_min: i32::MAX, x_max: i32::MIN, y_max: i32::MIN },
            ));
            guard.len() - 1
        }
    };
    let info = &mut guard[pos].1;
    info.draws += 1;
    if x < info.x_min {
        info.x_min = x;
    }
    if y < info.y_min {
        info.y_min = y;
    }
    if x > info.x_max {
        info.x_max = x;
    }
    if y > info.y_max {
        info.y_max = y;
    }
    let _ = anterior;
}

/// Vuelca las capas del frame y limpia el contador.
fn volcar_capas() {
    if std::env::var("XFACTORVR_DIAG_CAPAS").ok().as_deref() != Some("1") {
        return;
    }
    let mut guard = match CAPAS_FRAME.lock() {
        Ok(g) => g,
        Err(_) => return,
    };
    if !guard.is_empty() {
        let texto: Vec<String> = guard
            .iter()
            .map(|(c, i)| {
                format!(
                    "{c}:{} draws=[{},{}..{},{}]",
                    i.draws, i.x_min, i.y_min, i.x_max, i.y_max
                )
            })
            .collect();
        log::info(&format!("capas del frame: {}", texto.join(" | ")));
        guard.clear();
    }
    CAPA_ACTUAL.store(u32::MAX, Ordering::Relaxed);
}

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
    // El desplazamiento de camara va ANTES del original: el motor tiene que
    // calcular las posiciones de los sprites con la camara ya desplazada.
    aplicar_desplazamiento_camara(this);

    // La capa de este draw (para el GUI como textura).  La posicion se lee
    // como un MapPosition (2 x i32); en el GUI son pixeles de pantalla.
    if !position.is_null() {
        let mp = unsafe { std::ptr::read_unaligned(position as *const [i32; 2]) };
        anotar_capa(layer, mp[0], mp[1]);
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
    let entidad = con_estado(|e| e.entidad_de(pos));

    // Si la entidad tiene modelo 3D y se ha pedido sustituir el sprite, NO
    // se encola el sprite 2D (si no, la maquina se veria doble: el modelo
    // encima del sprite plano).
    let sustituir = match entidad {
        Some((_, m)) if m >= 0 && SIN_SPRITE.load(Ordering::Relaxed) == 1 => true,
        _ => false,
    };

    if !sustituir {
        // El original primero: el sprite tiene que existir.
        let tramp = TRAMPOLIN_DQ.load(Ordering::Relaxed);
        if tramp != 0 {
            // SAFETY: el trampolin repone los argumentos intactos.
            unsafe {
                let f: HookDrawQueue = std::mem::transmute(tramp);
                f(this, sprite, position, flags, layer, shift, _size_x, _size_y);
            }
        }
    }

    let (direccion, modelo) = match entidad {
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

    // El tamano del sprite.
    //
    // El `Sprite` que el motor pasa a `DrawQueue::draw` guarda su tamano en
    // dos `u16` en +0x16 y +0x18 (ver el desensamblado de `DrawQueue::draw`:
    // `ldrh w8, [x23, #0x16]`). Ese tamano es el del atlas, que va al doble
    // que el tamaño pintado, asi que se divide entre dos.
    let (sx, sy) = if sprite.is_null() {
        (0.0, 0.0)
    } else {
        // SAFETY: `sprite` es un `Sprite`; +0x16 y +0x18 son dos `u16` con el
        // ancho y el alto (verificado en el desensamblado del motor).
        unsafe {
            let p = sprite as usize;
            let w = std::ptr::read_unaligned((p + 0x16) as *const u16) as f64;
            let h = std::ptr::read_unaligned((p + 0x18) as *const u16) as f64;
            (w * 0.5 * escala, h * 0.5 * escala)
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
        modelo,
        // Pixels por casilla: el motor dibuja el mundo con 32 pixels por
        // casilla multiplicados por el zoom.
        escala: 32.0 * escala as f32,
    };
    con_estado(|e| e.anade_caja(caja));
    let total = PENDIENTES.fetch_add(1, Ordering::Relaxed) + 1;
    if total <= 20 {
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
    capturar_framebuffer_activo("sprites");
    pintar_si_toca("sprites");
}

/// Lee el framebuffer ACTIVO (el que esta bindeado ahora mismo) y lo guarda
/// como PNG.
///
/// El motor dibuja el GUI del juego en su PROPIO framebuffer, que solo esta
/// bindeado mientras lo dibuja (durante los flush).  Asi se puede leer el
/// GUI en el momento en que existe.
fn capturar_framebuffer_activo(sufijo: &str) {
    if std::env::var("XFACTORVR_CAPTURA_GUI").ok().as_deref() != Some("1") {
        return;
    }
    let Some(gl) = GL.get() else { return };
    let mut vp = [0i32; 4];
    unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
    let (w, h) = (vp[2], vp[3]);
    if w <= 0 || h <= 0 || w > 4096 || h > 4096 {
        return;
    }
    let Ok(ruta) = std::env::var("XFACTORVR_CAPTURA_GUI_RUTA") else {
        return;
    };
    if ruta.is_empty() {
        return;
    }
    let ruta = format!("{ruta}.{sufijo}");
    if capturar_png(gl, &vp, &ruta) {
        log::info(&format!("captura del framebuffer activo escrita en {ruta}"));
    }
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
    capturar_framebuffer_activo("instanced");
    pintar_si_toca("instanced");
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
    capturar_framebuffer_activo("primitives");
    pintar_si_toca("primitives");
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
    pintar_si_toca("text");
}

/// Pinta el cubo y hace la captura, si toca.
///
/// Solo se pinta en el framebuffer del mundo (el que no es el de la ventana):
/// ahi es donde el motor ha dibujado la escena y donde luego la compone.
fn pintar_si_toca(_origen: &str) {
    // El hook sirve para dos cosas: pintar el cubo 3D y mandar la imagen real
    // del juego al visor. Cada una puede estar activada por separado.
    let quiere_3d = ACTIVO.load(Ordering::Relaxed) != 0;
    let quiere_stream = crate::stream::activo();
    if (!quiere_3d && !quiere_stream) || USAR_GL.load(Ordering::Relaxed) == 0 {
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
    // El minimapa tambien es un DrawQueue (con su propia escala): ahi el
    // cubo sale gigante.  El framebuffer del mundo tiene la forma de la
    // ventana, asi que se descarta el que sea casi cuadrado.
    let aspecto = vp[2] as f32 / vp[3].max(1) as f32;
    if aspecto < 1.2 || aspecto > 2.4 {
        return;
    }

    // El framebuffer del mundo: es donde el motor compone la escena (y lo
    // que hay que leer para tener el frame completo).
    FB_MUNDO.store(fb as u64, Ordering::Relaxed);

    if DIAG_GL.swap(1, Ordering::Relaxed) == 0 {
        let muestras = gl_int(gl, GL_SAMPLES);
        log::info(&format!(
            "diagnostico GL: framebuffer={fb} viewport={}x{} muestras={muestras}",
            vp[2], vp[3]
        ));
    }

    if quiere_3d {
        dibujar_3d(gl, &vp);
    }
    capturar_si_pide(gl, &vp);
}

fn leer_pixeles(gl: &Gl, w: i32, h: i32, buf: &mut [u8]) {
    let fb_dibujo = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING);
    let fb_lectura = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING);
    let reenlazar = fb_dibujo != 0 && fb_dibujo != fb_lectura;
    if reenlazar {
        unsafe { (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fb_dibujo as u32) };
    }
    unsafe {
        (gl.pixel_storei)(GL_UNPACK_ALIGNMENT, 1);
        (gl.read_pixels)(
            0,
            0,
            w,
            h,
            GL_RGBA,
            GL_UNSIGNED_BYTE,
            buf.as_mut_ptr() as *mut c_void,
        );
        (gl.finish)();
    }
    if reenlazar {
        unsafe { (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fb_lectura as u32) };
    }
}


/// Invierte las filas de un buffer RGBA en su sitio (de OpenGL a coordenadas estandar).
fn voltear_filas_rgba(buf: &mut [u8], w: usize, h: usize) {
    if w == 0 || h <= 1 {
        return;
    }
    let stride = w * 4;
    if buf.len() < stride * h {
        return;
    }
    let mitad = h / 2;
    for y in 0..mitad {
        let opuesto = h - 1 - y;
        let fila_a = y * stride;
        let fila_b = opuesto * stride;
        let (primera, segunda) = buf.split_at_mut(fila_b);
        primera[fila_a..fila_a + stride].swap_with_slice(&mut segunda[..stride]);
    }
}

/// Captura el frame final COMPLETO (Mundo + GUI nativa) justo en el cierre
/// del frame (hook de swap) y lo transmite al visor VR.
pub fn capturar_frame_final() {
    volcar_capas();
    if !crate::stream::toca_enviar() {
        return;
    }

    preparar_gl();
    let gl = match GL.get() {
        Some(g) => g,
        None => return,
    };

    // Guardar los framebuffers actuales antes de que toquemos nada
    let fb_dibujo = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING) as u32;
    let fb_lectura = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING) as u32;

    drenar_errores(gl, "antes de capturar frame final");

    let blit_dst = ULTIMO_FB_BLIT_DESTINO.load(Ordering::Relaxed);
    let blit_src = ULTIMO_FB_BLIT_ORIGEN.load(Ordering::Relaxed);
    let fb_mundo = FB_MUNDO.load(Ordering::Relaxed) as u32;

    // Candidatos a leer, en orden de preferencia:
    // 1. fb_dibujo: donde el motor ha compuesto el frame final antes de swap
    // 2. fb_lectura: si hay un FBO de lectura activo
    // 3. blit_dst: FBO destino del ultimo blitTo (GUI)
    // 4. blit_src: FBO origen del ultimo blitTo
    // 5. 0: default framebuffer (ventana / pantalla)
    // 6. fb_mundo: fallback si nada mas funciona
    let candidatos = [fb_dibujo, fb_lectura, blit_dst, blit_src, 0, fb_mundo];

    let mut fb_elegido: Option<(u32, i32, i32)> = None;
    for &candidato in &candidatos {
        unsafe {
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, candidato);
        }
        let estado = check_fb_status(gl, GL_READ_FRAMEBUFFER);
        if estado != GL_FRAMEBUFFER_COMPLETE {
            continue;
        }

        let mut vp = [0i32; 4];
        unsafe { (gl.get_integerv)(GL_VIEWPORT, vp.as_mut_ptr()) };
        let w = vp[2];
        let h = vp[3];
        if w >= 64 && h >= 64 && w <= 4096 && h <= 4096 {
            fb_elegido = Some((candidato, w, h));
            break;
        }
    }

    let Some((fb, w, h)) = fb_elegido else {
        unsafe {
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, fb_dibujo);
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fb_lectura);
        }
        return;
    };

    let mut fb_final = fb;
    let mut w_final = w;
    let mut h_final = h;

    let bytes = (w as usize) * (h as usize) * 4;
    thread_local! {
        static BUFFER: std::cell::RefCell<Vec<u8>> = const { std::cell::RefCell::new(Vec::new()) };
    }
    BUFFER.with(|b| {
        let mut buf = b.borrow_mut();
        if buf.len() != bytes {
            buf.clear();
            buf.resize(bytes, 0);
        }

        unsafe {
            (gl.pixel_storei)(GL_PACK_ALIGNMENT, 1);
            (gl.read_pixels)(
                0,
                0,
                w,
                h,
                GL_RGBA,
                GL_UNSIGNED_BYTE,
                buf.as_mut_ptr() as *mut c_void,
            );
            (gl.finish)();
        }

        // Si se leyo de FBO 0 y salio completamente negro (tipico de offscreen EGL sin surface),
        // intentar leer del FBO de blit si esta completo y tiene imagen
        let tiene_color = buf.iter().take(4000).any(|&x| x != 0);
        if !tiene_color && fb == 0 {
            for &fallback_fb in &[blit_dst, blit_src, fb_mundo] {
                if fallback_fb != 0 && fallback_fb != fb {
                    unsafe {
                        (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fallback_fb);
                    }
                    if check_fb_status(gl, GL_READ_FRAMEBUFFER) == GL_FRAMEBUFFER_COMPLETE {
                        let mut fvp = [0i32; 4];
                        unsafe { (gl.get_integerv)(GL_VIEWPORT, fvp.as_mut_ptr()) };
                        let fw = fvp[2];
                        let fh = fvp[3];
                        if fw >= 64 && fh >= 64 {
                            let fbytes = (fw as usize) * (fh as usize) * 4;
                            if buf.len() != fbytes {
                                buf.resize(fbytes, 0);
                            }
                            unsafe {
                                (gl.read_pixels)(
                                    0,
                                    0,
                                    fw,
                                    fh,
                                    GL_RGBA,
                                    GL_UNSIGNED_BYTE,
                                    buf.as_mut_ptr() as *mut c_void,
                                );
                                (gl.finish)();
                            }
                            if buf.iter().take(4000).any(|&x| x != 0) {
                                log::info(&format!("fallback exitoso a FBO {fallback_fb} ({fw}x{fh})"));
                                fb_final = fallback_fb;
                                w_final = fw;
                                h_final = fh;
                                break;
                            }
                        }
                    }
                }
            }
        }

        crate::entrada::fijar_viewport(w_final as u32, h_final as u32);

        // Restaurar estado de framebuffers
        unsafe {
            (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, fb_dibujo);
            (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fb_lectura);
        }

        drenar_errores(gl, "despues de leer frame final");

        // OpenGL glReadPixels lee de abajo a arriba (fila 0 = fondo de la ventana).
        // Se invierten las filas en su sitio para que el frame transmitido empiece
        // en la fila 0 (arriba de la ventana), coincidiendo con SDL y formatos de imagen.
        voltear_filas_rgba(&mut buf, w_final as usize, h_final as usize);

        if crate::stream::enviar(&buf, w_final as u32, h_final as u32, ojo_actual()) {
            static LOG_N: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let n = LOG_N.fetch_add(1, Ordering::Relaxed);
            if n % 60 == 0 || n < 5 {
                log::info(&format!(
                    "frame final transmitido (FBO {fb_final}): {w_final}x{h_final} (enviados={}, perdidos={}, vacias={})",
                    crate::stream::enviados(),
                    crate::stream::perdidos(),
                    crate::stream::vacias()
                ));
            }
        }

        // Enviar tambien geometria por compatibilidad si hay modelos activos
        crate::stream::enviar_geometria(
            &entidades_visibles(),
            &crate::render3d::nombres_de_prototipos(),
            ojo_actual(),
        );
    });
}

pub fn capturar_frame_completo() {
    capturar_frame_final();
}

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
///   ```text
///     M = T(base) * Rx(inclinacion) * Rz(direccion) * S * T(0, 0, 1/2)
///   ```
///
///   * `Rz(direccion)` es la orientacion real de la entidad (16 pasos de
///     22,5 grados),
///   * `Rx(inclinacion)` inclina el cubo para que se vea su volumen con la
///     proyeccion ortografica del motor,
///   * `S` es el tamano real de la maquina (el de su sprite),
///   * la traslacion en Z deja la base del cubo en el suelo, en la posicion
///     de la entidad.
///
/// La `base` es el punto donde se apoya el cubo: el centro en X de la entidad
/// y el BORDE INFERIOR de su sprite (en el motor la Y crece hacia abajo, asi
/// que el suelo esta en `wy + alto/2`).
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

    // Base del cubo: el centro en X y el borde inferior del sprite.
    let base_x = c.wx;
    let base_y = c.wy + c.sy * 0.5;

    // R = Rx * Rz, en column-major:
    //   col0 = (cy,  ct*sy, st*sy)
    //   col1 = (-sy, ct*cy, st*cy)
    //   col2 = (0,   -st,   ct  )
    // Con la escala aplicada a cada columna y la traslacion al final.
    // El `+ 0.5` en Z (la traslacion de `T(0,0,1/2)`) deja la base del cubo
    // en el suelo: se suma la mitad de la columna Z a la traslacion.
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
        base_x,
        // Al inclinarlo, su componente vertical (y) sube en pantalla (y crece
        // hacia abajo), de ahi el `- st * ez * 0.5`.
        base_y - st * ez * 0.5,
        ct * ez * 0.5,
        1.0,
    ]
}

/// Calcula los vertices del cubo ya proyectados (la transformacion 3D y la
/// proyeccion de la camara se hacen aqui, en la CPU) y devuelve el buffer de
/// vertices (posicion en clip + normal en el mundo).
///
/// La posicion sale en coordenadas de clip (`-1..1`), que es lo que espera el
/// vertex shader: `gl_Position = vec4(position.xy, 0, 1)`.
fn vertices_cubo(c: &Caja, vp: &[i32; 4]) -> [f32; 144] {
    let m = matriz_cubo(c);
    let p = proyeccion(vp);
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

        // Proyeccion de la camara (ortografica, con la Y hacia abajo).
        let ndc_x = x * p[0] + p[12];
        let ndc_y = y * p[5] + p[13];

        // Normal transformada (sin traslacion): se queda en el mundo, que es
        // donde esta la luz.
        let ax = m[0] * nx + m[4] * ny + m[8] * nz;
        let ay = m[1] * nx + m[5] * ny + m[9] * nz;
        let az = m[2] * nx + m[6] * ny + m[10] * nz;
        let largo = (ax * ax + ay * ay + az * az).sqrt().max(1e-6);

        out[v * 6] = ndc_x;
        out[v * 6 + 1] = ndc_y;
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

/// Dibuja el 3D en el framebuffer que hay enlazado.
///
/// Primero los **modelos de verdad** (los GLB) y, si esta puesto el modo
/// cubo, tambien el cubo de prueba (que sirve de referencia).
fn dibujar_3d(gl: &Gl, vp: &[i32; 4]) {
    let cajas = con_estado(|e| e.publica_cajas());
    if cajas.is_empty() {
        return;
    }
    // El throttle: solo se dibujan los modelos cada N flushes.
    let n = CONTADOR_FLUSH.fetch_add(1, Ordering::Relaxed);
    let cada = CADA_FLUSH.load(Ordering::Relaxed).max(1);
    if n % cada == 0 {
        dibujar_modelos(&cajas, vp);
    }
    if CUBO_ACTIVO.load(Ordering::Relaxed) == 0 {
        return;
    }
    dibujar_cajas(gl, vp, &cajas);
}

/// Dibuja los modelos 3D de verdad (una instancia por entidad).
fn dibujar_modelos(cajas: &[Caja], vp: &[i32; 4]) {
    let mut instancias: Vec<crate::render3d::Instancia> = Vec::new();
    for c in cajas.iter() {
        if c.modelo < 0 {
            // Sin modelo: no se dibuja nada (la entidad se sigue viendo con
            // su sprite normal: no queda un hueco).
            continue;
        }
        if c.sx <= 0.0 || c.sy <= 0.0 {
            continue;
        }
        // Las entidades fuera de la pantalla no se dibujan.
        if c.wx + c.sx * 0.5 < 0.0
            || c.wx - c.sx * 0.5 > vp[2] as f32
            || c.wy + c.sy * 0.5 < 0.0
            || c.wy - c.sy * 0.5 > vp[3] as f32
        {
            continue;
        }
        instancias.push(crate::render3d::Instancia {
            modelo: c.modelo as usize,
            x: c.wx,
            // El pie de la maquina se apoya en el borde inferior de su sprite.
            y: c.wy + c.sy * 0.5,
            direccion: c.direccion,
            pixeles_por_tile: c.escala,
        });
    }
    if instancias.is_empty() {
        return;
    }
    if instancias.len() > MAX_INSTANCIAS {
        instancias.truncate(MAX_INSTANCIAS);
    }
    // De lejos a cerca: las mas altas en la pantalla (menor Y) se dibujan
    // primero (no hay buffer de profundidad: el orden lo decide el pintón).
    instancias.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap_or(std::cmp::Ordering::Equal));
    crate::render3d::dibujar(&instancias, vp);
}

/// Dibuja las cajas pendientes en el framebuffer que hay enlazado.
fn dibujar_cajas(gl: &Gl, vp: &[i32; 4], cajas: &[Caja]) {
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
    // El motor dibuja el mundo por trozos con el recorte (scissor) activado:
    // si esta puesto, el cubo solo se ve en el trozo que se este pintando en
    // ese momento (y casi nunca).  Se ensancha la caja de recorte a todo el
    // viewport mientras se dibuja el cubo y despues se devuelve la que
    // tenia: asi el motor sigue viendo EXACTAMENTE el mismo estado (ni
    // siquiera hay que saber si el recorte estaba activo).
    let mut caja_recorte = [0i32; 4];
    unsafe { (gl.get_integerv)(GL_SCISSOR_BOX, caja_recorte.as_mut_ptr()) };

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
        // Una caja mas grande que la pantalla es una lectura erroneous del
        // tamano del sprite: si se dibuja, tapa media imagen de magenta.
        if c.sx > vp[2] as f32 || c.sy > vp[3] as f32 {
            continue;
        }
        // Las entidades que estan fuera de la pantalla no se pintan: si se
        // pintaran, el cubo se veria recortado en el borde del framebuffer y
        // la captura daria pixeles "del cubo" que no son de ninguna entidad
        // visible.
        if c.wx + c.sx * 0.5 < 0.0
            || c.wx - c.sx * 0.5 > vp[2] as f32
            || c.wy + c.sy * 0.5 < 0.0
            || c.wy - c.sy * 0.5 > vp[3] as f32
        {
            continue;
        }
        let vertices = vertices_cubo(&c, vp);
        let m = matriz_cubo(&c);
        let indices = orden_caras(&m);

        unsafe {
            // Recorte a todo el viewport (el cubo tiene que verse entero).
            (gl.scissor)(0, 0, vp[2], vp[3]);
            (gl.use_program)(cubo.programa);
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
            // Donde ha quedado el cubo en el framebuffer (en pixeles), para
            // comprobarlo a ojo en la captura: `mundo` ya son pixeles del
            // framebuffer del mundo.
            log::info(&format!(
                "cubo 3D en pantalla: ({:.1}, {:.1}) de {}x{}  [mundo=({:.1}, {:.1}) tam={:.1}x{:.1}]",
                c.wx,
                c.wy + c.sy * 0.5,
                vp[2],
                vp[3],
                c.wx,
                c.wy,
                c.sx,
                c.sy
            ));
        }
    }

    // Se devuelve el estado al motor.
    unsafe {
        // La caja de recorte, tal y como estaba.
        (gl.scissor)(caja_recorte[0], caja_recorte[1], caja_recorte[2], caja_recorte[3]);
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
}

// --------------------------------------------------------------------------
// Camara: la proyeccion la calculamos nosotros
// --------------------------------------------------------------------------

/// La proyeccion de la camara del motor.
///
/// El motor dibuja el mundo con una proyeccion ortografica. Las
/// coordenadas del mundo (`mundo`) son la posicion del sprite en el
/// framebuffer del mundo, en pixeles (verificado con una maquina creada en
/// el juego: su `mundo` era (856, 384) y en la captura salia en esos mismos
/// pixeles).
///
/// El framebuffer del mundo se dibuja con la Y hacia abajo (al componerlo en
/// la ventana el motor lo voltea): al volcarlo con `glReadPixels` sale
/// derecho, con la GUI abajo. Por eso la Y del clip crece hacia abajo.
///
/// La cuenta es la de siempre:
///
///   ```text
///     ndc_x = mundo_x * 2 / W - 1
///     ndc_y = mundo_y * 2 / H - 1
///   ```
///
/// con W y H los del viewport del framebuffer del mundo.
///
/// Se devuelve encolumnada por columnas (como la espera GL).
fn proyeccion(vp: &[i32; 4]) -> [f32; 16] {
    let w = vp[2].max(1) as f32;
    let h = vp[3].max(1) as f32;
    [
        2.0 / w, 0.0, 0.0, 0.0, //
        0.0, 2.0 / h, 0.0, 0.0, //
        0.0, 0.0, 0.0, 0.0, //
        -1.0, -1.0, 0.0, 1.0, //
    ]
}

/// Hook de `GraphicsInterfaceOpenGL::bindUniformBuffer`.
///
/// Firma: `bindUniformBuffer(uint index, PipelineStateObject const*,
/// UniformBuffer const*)`.
///
/// Desensamblado del motor (`0x34ea070`): el bloque enlazado es
/// `pso->[0x78][index]` y el buffer de GL es `ub->[0x10]`.
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
    // Apuntar lo que se ve. **Nada de GL aqui**: preguntar al motor por sus
    // programas lo hacia petar (ver el aviso de arriba).
    if !uniform_buffer.is_null() && !pipeline.is_null() && index < 256 {
        // SAFETY: offsets verificados en el desensamblado del motor
        // (`0x34ea070`): el buffer de GL esta en `ub+0x10`, el programa en
        // `pso+0x20` y la tabla de bloques en `pso+0x78`.
        let (gl_buffer, programa, tabla) = unsafe {
            (
                std::ptr::read_unaligned((uniform_buffer as usize + 0x10) as *const u32),
                std::ptr::read_unaligned((pipeline as usize + 0x20) as *const u32),
                std::ptr::read_unaligned((pipeline as usize + 0x78) as *const usize),
            )
        };
        if gl_buffer != 0 && programa != 0 && tabla != 0 {
            // SAFETY: `index` es el indice dentro de la tabla de bloques del
            // pipeline state (la comprobacion de `index < 256` lo acota).
            let bloque =
                unsafe { std::ptr::read_unaligned((tabla + index as usize * 4) as *const u32) };
            if DIAG_UB.swap(1, Ordering::Relaxed) == 0 {
                log::info(&format!(
                    "bindUniformBuffer: index={index} buffer={gl_buffer} programa={programa} \
                     bloque={bloque} (solo diagnostico: no se toca la GL)"
                ));
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

/// Compara el framebuffer del mundo con el de la pantalla (FBO 0).
///
/// El motor compone el GUI del juego en la pantalla: si la pantalla y el
/// mundo se diferencian, lo que se ve ahi es el GUI (hotbar, minimapa...).
/// Deja constancia de cuantos pixeles difieren y guarda la pantalla como
/// PNG si se pide (XFACTORVR_CAPTURA_PANTALLA).
#[allow(dead_code)]
fn comparar_con_la_pantalla(gl: &Gl, mundo: &[u8], w: i32, h: i32) {
    let fb_dibujo = gl_int(gl, GL_DRAW_FRAMEBUFFER_BINDING);
    let fb_lectura = gl_int(gl, GL_READ_FRAMEBUFFER_BINDING);
    // El framebuffer 0 (la pantalla por defecto).
    unsafe {
        (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, 0);
        (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, 0);
    }
    let mut pantalla = vec![0u8; (w as usize) * (h as usize) * 4];
    leer_pixeles(gl, w, h, &mut pantalla);
    drenar_errores(gl, "leer la pantalla");
    unsafe {
        (gl.bind_framebuffer)(GL_DRAW_FRAMEBUFFER, fb_dibujo as u32);
        (gl.bind_framebuffer)(GL_READ_FRAMEBUFFER, fb_lectura as u32);
    }

    let mut difieren = 0u64;
    let mut con_color = 0u64;
    for (a, b) in mundo.chunks_exact(4).zip(pantalla.chunks_exact(4)) {
        if a != b {
            difieren += 1;
        }
        if b[0] != 0 || b[1] != 0 || b[2] != 0 {
            con_color += 1;
        }
    }
    log::info(&format!(
        "pantalla vs mundo: {} de {} pixeles difieren (pantalla con color: {})",
        difieren,
        (w as u64) * (h as u64),
        con_color
    ));

    if let Ok(ruta) = std::env::var("XFACTORVR_CAPTURA_PANTALLA") {
        if !ruta.is_empty() {
            let vp = [0i32, 0, w, h];
            if capturar_png(gl, &vp, &ruta) {
                log::info(&format!("captura de la PANTALLA escrita en {ruta}"));
            }
        }
    }
}

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

/// Cuenta los pixeles que son del cubo.
///
/// El cubo se pinta de **magenta** (`0.90, 0.10, 0.75` en el shader) y en la
/// escena del Factorio no hay nada de ese color: las llamas son amarillas,
/// la tierra marron, el jugador azul y la GUI gris. Asi que un pixel con
/// mucho rojo y mucho azul, y poco verde, es del cubo.
///
/// Sirve para comprobar automaticamente que el cubo esta en la captura.
fn pixeles_del_cubo(pix: &[u8]) -> u64 {
    let mut n = 0u64;
    for p in pix.chunks_exact(4) {
        let (r, g, b) = (p[0] as i32, p[1] as i32, p[2] as i32);
        if r >= 60 && b >= 60 && (r - g) >= 40 && (b - g) >= 30 {
            n += 1;
        }
    }
    n
}

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
    leer_pixeles(gl, w, h, &mut pix);
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
    let del_cubo = pixeles_del_cubo(&pix);
    log::info(&format!(
        "captura {w}x{h}: {} pixeles con color, {} opacos, {} negros, {} del cubo",
        con_contenido,
        opacos,
        (w as u64) * (h as u64) - con_contenido,
        del_cubo
    ));

    // La captura solo se escribe si se ve el cubo: asi el PNG del banco es
    // prueba de que el cubo esta de verdad en la imagen (y no solo de que el
    // juego no se ha caido).
    if del_cubo == 0 {
        return false;
    }

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
    // Adler-32 del bloque sin comprimir. OJO: son dos `u16` (4 bytes), no dos
    // `u32`: con `u32` el PNG salia con 4 bytes de mas y el checksum roto (y
    // python no lo podia leer).
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in &raw {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    z.extend_from_slice(&(b as u16).to_be_bytes());
    z.extend_from_slice(&(a as u16).to_be_bytes());
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

    // --- hooks de las entidades con 3D ---
    //
    // Cada tipo tiene su propio simbolo (y su propio trampolin).  Si uno no
    // existe en esta version del motor, se sigue con los demas.
    let mut enganchados = 0usize;
    for (i, (mangle, nombre)) in TIPOS_3D.iter().enumerate() {
        if i >= MAX_TIPOS_3D || i >= HOOKS_ENT.len() {
            break;
        }
        let encontrado = match simbolos.resolver_exacto(mangle) {
            Some(r) => r,
            None => {
                log::aviso(&format!("no se encontro {nombre}::draw: ese tipo no lleva 3D"));
                continue;
            }
        };
        log::info(&format!(
            "resuelto: {nombre}::draw() -> 0x{:x} (mangled {})",
            encontrado.direccion, encontrado.mangled
        ));

        // Direccion de ejecucion del hook (dlsym, para no coger la de enlace).
        let simbolo_hook = format!("xfv_hook_ent_{i}");
        let hook_addr = match direccion_de_ejecucion_de(&simbolo_hook) {
            Some(a) => a,
            None => {
                log::aviso(&format!("dlsym no devolvio el hook de {nombre}; se usa la direccion de enlace"));
                HOOKS_ENT[i] as *const () as u64
            }
        };

        match detour::instalar(encontrado.direccion, hook_addr, None, encontrado.tamano) {
            Ok(d) => {
                TRAMPOLIN_ENT[i].store(d.trampolin as usize, Ordering::SeqCst);
                enganchados += 1;
                log::info(&format!(
                    "detour instalado sobre {nombre}::draw(): trampolin en 0x{:x}",
                    d.trampolin
                ));
            }
            Err(e) => {
                log::error(&format!("no se pudo instalar el detour de {nombre}::draw: {e}"));
            }
        }
    }
    if enganchados == 0 {
        log::error("no se pudo enganchar ninguna entidad: no habra 3D");
        return;
    }
    log::info(&format!("3D: {enganchados} tipos de entidad enganchados"));

    // Las funciones GL del motor: solo se resuelven sus direcciones; los
    // punteros los carga GLAD mas tarde (ver `preparar_gl`).
    if let Some(r) = simbolos
        .resolver_exacto("SDL_GL_GetProcAddress_REAL")
        .or_else(|| simbolos.resolver("SDL_GL_GetProcAddress"))
    {
        SDL_GET_PROC_ADDRESS.store(r.direccion as usize, Ordering::Relaxed);
        log::info(&format!("SDL_GL_GetProcAddress resuelto en 0x{:x}", r.direccion));
    }
    match resolver_gl(simbolos) {
        Some(d) => {
            log::info("funciones GL del motor localizadas (los punteros se leen al primer frame)");
            let _ = GL_DIR.set(d);
        }
        None => {
            log::aviso("no se encontraron todas las funciones GL: no se dibujaran cubos");
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

    // --- hook del blit del framebuffer (para capturar el GUI del juego) ---
    const MANGLE_BLIT: &str = "_ZN17FramebufferOpenGL6blitToER11VideoBitmapiiiiii";
    if let Some(blit) = simbolos.resolver_exacto(MANGLE_BLIT) {
        if let Some(a) = direccion_de_ejecucion_de("xfv_hook_blit_to") {
            match detour::instalar(blit.direccion, a, None, blit.tamano) {
                Ok(d) => {
                    TRAMPOLIN_BLIT.store(d.trampolin as usize, Ordering::SeqCst);
                    log::info("detour instalado sobre FramebufferOpenGL::blitTo()");
                }
                Err(e) => log::error(&format!("no se pudo instalar el detour de blitTo: {e}")),
            }
        }
    } else {
        log::aviso("no se encontro FramebufferOpenGL::blitTo()");
    }

    // --- hook de la captura de pantalla del motor (el frame con el GUI) ---
    const MANGLE_SCREEN: &str =
        "_ZN10RenderUtil15takeScreenshotsERKSt6vectorI17ScreenshotRequestSaIS1_EER3Mapb";
    if let Some(sc) = simbolos.resolver_exacto(MANGLE_SCREEN) {
        if let Some(a) = direccion_de_ejecucion_de("xfv_hook_take_screenshots") {
            match detour::instalar(sc.direccion, a, None, sc.tamano) {
                Ok(d) => {
                    TRAMPOLIN_SCREEN.store(d.trampolin as usize, Ordering::SeqCst);
                    log::info("detour instalado sobre RenderUtil::takeScreenshots()");
                }
                Err(e) => log::error(&format!("no se pudo instalar el detour de takeScreenshots: {e}")),
            }
        }
    } else {
        log::aviso("no se encontro RenderUtil::takeScreenshots()");
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

    // El renderer de los modelos de verdad (usa el mismo contexto GL).
    crate::render3d::preparar_gl();
}

/// Devuelve un resumen de lo que ha pasado (para el log).
pub fn resumen() -> String {
    format!(
        "draw {} veces, {} cajas pendientes, {} cubos dibujadas, {} mallas de modelo 3D, \
         {} prototipos identificados",
        LLAMADAS.load(Ordering::Relaxed),
        PENDIENTES.load(Ordering::Relaxed),
        DIBUJADOS.load(Ordering::Relaxed),
        crate::render3d::dibujados(),
        crate::proto::IDENTIFICADOS.load(Ordering::Relaxed),
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
    if let Ok(v) = std::env::var("XFACTORVR_3D_CAMARA") {
        if let Ok(d) = v.trim().parse::<f64>() {
            if d.is_finite() {
                DESPLAZAMIENTO_CAMARA.store(d.to_bits(), Ordering::Relaxed);
            }
        }
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_VOLTEAR") {
        CAPTURAR_VOLTEADA.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_CUBO") {
        CUBO_ACTIVO.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_SIN_MODELOS") {
        SIN_MODELOS.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_SIN_SPRITE") {
        SIN_SPRITE.store(if v != "0" { 1 } else { 0 }, Ordering::Relaxed);
    }
    if let Ok(v) = std::env::var("XFACTORVR_3D_CADA_FLUSH") {
        if let Ok(n) = v.trim().parse::<u64>() {
            CADA_FLUSH.store(n.max(1), Ordering::Relaxed);
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// El PNG que escribe la captura tiene que ser valido: el stream zlib
    /// lleva su Adler-32 y cada bloque del PNG lleva su CRC-32.  Si uno de los
    /// dos esta mal, el fichero no lo puede leer nadie (y el banco no puede
    /// comprobar si el cubo esta).
    #[test]
    fn el_png_que_escribimos_es_valido() {
        let w = 7u32;
        let h = 5u32;
        let mut pix = Vec::with_capacity((w * h * 4) as usize);
        for i in 0..(w * h * 4) {
            pix.push((i * 7 % 251) as u8);
        }
        let ruta = std::env::temp_dir().join("xfv_prueba_png.png");
        assert!(escribir_png(ruta.to_str().unwrap(), w, h, &pix));

        let datos = std::fs::read(&ruta).expect("no se pudo leer el PNG");
        assert_eq!(&datos[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        // Recorrer los bloques y comprobar el CRC de cada uno.
        let mut pos = 8usize;
        let mut idat = Vec::new();
        let mut visto_iend = false;
        while pos + 8 <= datos.len() {
            let largo = u32::from_be_bytes([
                datos[pos], datos[pos + 1], datos[pos + 2], datos[pos + 3],
            ]) as usize;
            let tipo = &datos[pos + 4..pos + 8];
            let contenido = &datos[pos + 8..pos + 8 + largo];
            let crc_leido = u32::from_be_bytes([
                datos[pos + 8 + largo],
                datos[pos + 9 + largo],
                datos[pos + 10 + largo],
                datos[pos + 11 + largo],
            ]);
            let mut para_crc = tipo.to_vec();
            para_crc.extend_from_slice(contenido);
            assert_eq!(
                crc32(&para_crc),
                crc_leido,
                "CRC malo en el bloque {:?}",
                String::from_utf8_lossy(tipo)
            );
            if tipo == b"IDAT" {
                idat = contenido.to_vec();
            }
            if tipo == b"IEND" {
                visto_iend = true;
            }
            pos += 12 + largo;
        }
        assert!(visto_iend, "falta el bloque IEND");

        // El Adler-32 del final del stream zlib tiene que ser el de los datos.
        let (mut a, mut b) = (1u32, 0u32);
        let mut p = 2usize; // cabecera zlib
        while p < idat.len() {
            let fin = idat[p];
            p += 1;
            let n = u16::from_le_bytes([idat[p], idat[p + 1]]) as usize;
            p += 4; // LEN + NLEN
            for &byte in &idat[p..p + n] {
                a = (a + byte as u32) % 65521;
                b = (b + a) % 65521;
            }
            p += n;
            if fin == 1 {
                break;
            }
        }
        let esperado = ((b << 16) | a).to_be_bytes();
        assert_eq!(
            &idat[idat.len() - 4..],
            &esperado,
            "el Adler-32 del stream zlib no cuadra"
        );
        let _ = std::fs::remove_file(&ruta);
    }
}

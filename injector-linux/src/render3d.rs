//! Dibujado de los **modelos 3D de verdad** (los GLB) dentro del framebuffer
//! del mundo del motor.
//!
//! # Contexto GL
//!
//! Se usa **el contexto del propio motor**: no se crea ninguno.  Los punteros
//! a las funciones de GL se sacan con `dlsym` de la libreria de GL que ya
//! tiene cargada el juego (`libGLESv2.so` en Android, `libGL.so.1` en el
//! banco).  El motor se apoya en GLAD, pero GLAD no expone
//! `glUniformMatrix4fv` (el motor usa bloques de uniforms), asi que para las
//! matrices hay que ir a la libreria de GL.
//!
//! # Proyeccion
//!
//! El motor dibuja el mundo con una proyeccion **ortografica en X e Y** (el
//! suelo es un plano).  Para que un modelo 3D se vea con volumen se le aplica
//! un **inclinado** (rotacion alrededor del eje X) y se proyecta al plano de
//! la pantalla:
//!
//! ```text
//!   pantalla_x = x
//!   pantalla_y = -cos(inclinacion) * altura + sin(inclinacion) * profundidad
//! ```
//!
//! El resultado se traduce a coordenadas de clip con la misma cuenta que usa
//! el motor para sus sprites (`ndc = pixel * 2 / viewport - 1`), asi que el
//! modelo cae **encima** del sprite de la entidad y se ve el volumen de la
//! maquina.
//!
//! El modelo se coloca con el pie en el suelo de la entidad (su `Y = 0`) y
//! centrado en su huella (se recentra al cargarlo), y se gira con la
//! direccion de la entidad (16 pasos de 22,5 grados, como en el mod).

use std::ffi::c_void;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::glb::{self, Modelo};
use crate::log;
use crate::modelos::{self, Giro, Manifiesto};

/// Pixels por casilla del motor (a escala 1.0).
///
/// El motor dibuja el mundo con `mundo = (mapa / 256) * (32 * escala) - camara`:
/// una casilla son 32 pixels multiplicados por el zoom.
pub const PIXELES_POR_TILE: f32 = 32.0;

/// Inclinacion de la vista (radianes). 0 = cenital, PI/2 = de frente.
const INCLINACION: f32 = 0.62;

/// Giro base del modelo respecto a la orientacion de la entidad.
///
/// 0 radianes = el modelo mira al norte (-Y de la pantalla), como en el mod
/// (`YAW_BASE` del crate del visor).
const YAW_BASE: f32 = core::f32::consts::PI;

// --------------------------------------------------------------------------
// Punteros a las funciones de GL
// --------------------------------------------------------------------------

type GlGetIntegerv = unsafe extern "C" fn(pname: u32, params: *mut i32);
type GlGetError = unsafe extern "C" fn() -> u32;
type GlGetString = unsafe extern "C" fn(name: u32) -> *const u8;
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
type GlGetUniformLocation = unsafe extern "C" fn(program: u32, name: *const i8) -> i32;
type GlUniformMatrix4fv =
    unsafe extern "C" fn(location: i32, count: i32, transpose: u8, value: *const f32);
type GlUniform2f = unsafe extern "C" fn(location: i32, v0: f32, v1: f32);
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
type GlScissor = unsafe extern "C" fn(x: i32, y: i32, width: i32, height: i32);
type GlDisable = unsafe extern "C" fn(cap: u32);
type GlEnable = unsafe extern "C" fn(cap: u32);
type GlBlendFuncSeparate = unsafe extern "C" fn(
    src_rgb: u32,
    dst_rgb: u32,
    src_alpha: u32,
    dst_alpha: u32,
);

/// Las funciones de GL que usa este modulo.
pub struct Gl {
    get_integerv: GlGetIntegerv,
    get_error: GlGetError,
    get_string: GlGetString,
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
    get_uniform_location: GlGetUniformLocation,
    uniform_matrix4fv: GlUniformMatrix4fv,
    uniform_2f: GlUniform2f,
    gen_buffers: GlGenBuffers,
    bind_buffer: GlBindBuffer,
    buffer_data: GlBufferData,
    gen_vertex_arrays: GlGenVertexArrays,
    bind_vertex_array: GlBindVertexArray,
    enable_vertex_attrib_array: GlEnableVertexAttribArray,
    vertex_attrib_pointer: GlVertexAttribPointer,
    draw_elements: GlDrawElements,
    scissor: GlScissor,
    disable: GlDisable,
    enable: GlEnable,
    blend_func_separate: GlBlendFuncSeparate,
}

/// Punteros a las funciones de GL (se rellenan en el primer frame).
static GL: OnceLock<Gl> = OnceLock::new();

/// Codigos de GL usados.
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
const GL_VERSION: u32 = 0x1F02;
const GL_RENDERER: u32 = 0x1F01;
const GL_VERTEX_ARRAY_BINDING: u32 = 0x85B5;
const GL_ARRAY_BUFFER_BINDING: u32 = 0x8894;

/// Resuelve una funcion de GL por su nombre.
///
/// Se prueba primero en el ambito global (el motor puede tenerla ahi) y si
/// no, se abre la libreria de GL del sistema.
fn resolver_gl(nombre: &str) -> Option<usize> {
    extern "C" {
        fn dlsym(handle: *mut core::ffi::c_void, symbol: *const core::ffi::c_char)
            -> *mut core::ffi::c_void;
        fn dlopen(file: *const core::ffi::c_char, mode: CIntT) -> *mut core::ffi::c_void;
    }
    type CIntT = i32;
    const RTLD_DEFAULT: *mut core::ffi::c_void = 0 as *mut core::ffi::c_void;
    const RTLD_NOW: i32 = 2;
    const RTLD_GLOBAL: i32 = 0x100;

    let mut c = [0u8; 64];
    if nombre.len() >= c.len() {
        return None;
    }
    c[..nombre.len()].copy_from_slice(nombre.as_bytes());
    let sym = c.as_ptr() as *const core::ffi::c_char;

    // SAFETY: `sym` es una cadena terminada en NUL valida.
    unsafe {
        let p = dlsym(RTLD_DEFAULT, sym);
        if !p.is_null() {
            return Some(p as usize);
        }
        for lib in [
            "libGLESv3.so",
            "libGLESv2.so",
            "libGL.so.1",
            "libGL.so",
        ] {
            let mut l = [0u8; 32];
            if lib.len() >= l.len() {
                continue;
            }
            l[..lib.len()].copy_from_slice(lib.as_bytes());
            let h = dlopen(l.as_ptr() as *const core::ffi::c_char, RTLD_NOW | RTLD_GLOBAL);
            if h.is_null() {
                continue;
            }
            let p = dlsym(h, sym);
            if !p.is_null() {
                return Some(p as usize);
            }
        }
    }
    None
}

/// Construye el struct de funciones de GL.
fn construir_gl() -> Option<Gl> {
    macro_rules! f {
        ($n:expr) => {
            match resolver_gl($n) {
                Some(a) => a,
                None => {
                    log::error(&format!("no se encontro la funcion de GL '{}'", $n));
                    return None;
                }
            }
        };
    }
    unsafe {
        Some(Gl {
            get_integerv: std::mem::transmute(f!("glGetIntegerv")),
            get_error: std::mem::transmute(f!("glGetError")),
            get_string: std::mem::transmute(f!("glGetString")),
            create_shader: std::mem::transmute(f!("glCreateShader")),
            shader_source: std::mem::transmute(f!("glShaderSource")),
            compile_shader: std::mem::transmute(f!("glCompileShader")),
            get_shaderiv: std::mem::transmute(f!("glGetShaderiv")),
            get_shader_info_log: std::mem::transmute(f!("glGetShaderInfoLog")),
            create_program: std::mem::transmute(f!("glCreateProgram")),
            attach_shader: std::mem::transmute(f!("glAttachShader")),
            link_program: std::mem::transmute(f!("glLinkProgram")),
            get_programiv: std::mem::transmute(f!("glGetProgramiv")),
            get_program_info_log: std::mem::transmute(f!("glGetProgramInfoLog")),
            use_program: std::mem::transmute(f!("glUseProgram")),
            get_uniform_location: std::mem::transmute(f!("glGetUniformLocation")),
            uniform_matrix4fv: std::mem::transmute(f!("glUniformMatrix4fv")),
            uniform_2f: std::mem::transmute(f!("glUniform2f")),
            gen_buffers: std::mem::transmute(f!("glGenBuffers")),
            bind_buffer: std::mem::transmute(f!("glBindBuffer")),
            buffer_data: std::mem::transmute(f!("glBufferData")),
            gen_vertex_arrays: std::mem::transmute(f!("glGenVertexArrays")),
            bind_vertex_array: std::mem::transmute(f!("glBindVertexArray")),
            enable_vertex_attrib_array: std::mem::transmute(f!("glEnableVertexAttribArray")),
            vertex_attrib_pointer: std::mem::transmute(f!("glVertexAttribPointer")),
            draw_elements: std::mem::transmute(f!("glDrawElements")),
            scissor: std::mem::transmute(f!("glScissor")),
            disable: std::mem::transmute(f!("glDisable")),
            enable: std::mem::transmute(f!("glEnable")),
            blend_func_separate: std::mem::transmute(f!("glBlendFuncSeparate")),
        })
    }
}

// --------------------------------------------------------------------------
// Shaders
// --------------------------------------------------------------------------

/// Vertex shader (OpenGL de escritorio).
const VS_DESKTOP: &str = r#"#version 330
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec3 aNrm;
layout(location = 2) in vec4 aCol;
uniform mat4 u_model;
uniform vec2 u_viewport;
out vec3 vNrm;
out vec4 vCol;
void main() {
    vec4 p = u_model * vec4(aPos, 1.0);
    gl_Position = vec4(p.x * 2.0 / u_viewport.x - 1.0, p.y * 2.0 / u_viewport.y - 1.0, 0.0, 1.0);
    vNrm = mat3(u_model) * aNrm;
    vCol = aCol;
}
"#;

/// Fragment shader (OpenGL de escritorio).
const FS_DESKTOP: &str = r#"#version 330
in vec3 vNrm;
in vec4 vCol;
out vec4 outColor;
void main() {
    vec3 n = normalize(vNrm);
    vec3 luz = normalize(vec3(0.35, 0.75, 0.55));
    float d = max(dot(n, luz), 0.0);
    vec3 c = vCol.rgb * (0.45 + 0.65 * d);
    outColor = vec4(c, vCol.a);
}
"#;

/// Vertex shader (OpenGL ES 3.0: Android).
const VS_ES: &str = r#"#version 300 es
layout(location = 0) in vec3 aPos;
layout(location = 1) in vec3 aNrm;
layout(location = 2) in vec4 aCol;
uniform mat4 u_model;
uniform vec2 u_viewport;
out vec3 vNrm;
out vec4 vCol;
void main() {
    vec4 p = u_model * vec4(aPos, 1.0);
    gl_Position = vec4(p.x * 2.0 / u_viewport.x - 1.0, p.y * 2.0 / u_viewport.y - 1.0, 0.0, 1.0);
    vNrm = mat3(u_model) * aNrm;
    vCol = aCol;
}
"#;

/// Fragment shader (OpenGL ES 3.0: Android).
const FS_ES: &str = r#"#version 300 es
precision mediump float;
in vec3 vNrm;
in vec4 vCol;
out vec4 outColor;
void main() {
    vec3 n = normalize(vNrm);
    vec3 luz = normalize(vec3(0.35, 0.75, 0.55));
    float d = max(dot(n, luz), 0.0);
    vec3 c = vCol.rgb * (0.45 + 0.65 * d);
    outColor = vec4(c, vCol.a);
}
"#;

/// Programa de los modelos (con la localizacion de sus uniforms).
struct Programa {
    programa: u32,
    loc_model: i32,
    loc_viewport: i32,
}

/// Programa y buffers (se crean en el primer frame con contexto GL).
static PROGRAMA: OnceLock<Option<Programa>> = OnceLock::new();

// --------------------------------------------------------------------------
// Modelos
// --------------------------------------------------------------------------

/// Una malla subida a la GPU.
#[derive(Clone, Copy)]
struct MallaGpu {
    vao: u32,
    indices: usize,
}

/// Un modelo en la GPU.
#[derive(Clone)]
struct ModeloGpu {
    mallas: Vec<MallaGpu>,
}

/// Un modelo en la CPU (aun sin subir).
struct ModeloCpu {
    /// Las mallas del GLB (posicion/normales/color e indices).
    modelo: Modelo,
    /// Casillas por unidad del GLB.
    casillas_por_unidad: f32,
}

/// Una instancia a dibujar.
#[derive(Clone, Copy, Debug)]
pub struct Instancia {
    /// Indice del modelo en el registro.
    pub modelo: usize,
    /// Posicion en pixeles del framebuffer del mundo (x, y del suelo).
    pub x: f32,
    pub y: f32,
    /// Direccion de la entidad (0..15).
    pub direccion: u8,
    /// Pixels por casilla (el zoom del motor).
    pub pixeles_por_tile: f32,
}

/// El registro de modelos: prototipo -> modelo (en CPU y en GPU).
#[derive(Default)]
struct Registro {
    /// Nombres de prototipo, en el orden de los indices.
    prototipos: Vec<String>,
    /// Modelos en CPU (se cargan del disco la primera vez que se usan).
    cpu: Vec<Option<ModeloCpu>>,
    /// Modelos en la GPU (se suben la primera vez que se dibujan).
    gpu: Vec<Option<ModeloGpu>>,
    /// Manifiesto cargado.
    manifiesto: Option<Manifiesto>,
}

static REGISTRO: OnceLock<Mutex<Registro>> = OnceLock::new();

/// El registro (con el mutex).
fn registro() -> std::sync::MutexGuard<'static, Registro> {
    let r = REGISTRO.get_or_init(|| Mutex::new(Registro::default()));
    r.lock().unwrap_or_else(|p| p.into_inner())
}

/// Los nombres de prototipo del registro, en el orden de sus indices.
///
/// Es la tabla que viaja al visor junto con la geometria (ADR-021): los dos
/// lados numeran los modelos de forma distinta (aqui el indice en el vector
/// de prototipos, en el visor un id contador), asi que el visor recibe el
/// NOMBRE y lo convierte con su propio `id_de_prototipo`.
pub fn nombres_de_prototipos() -> Vec<String> {
    registro().prototipos.clone()
}

/// Carga el manifiesto (una sola vez) y deja el registro listo.
pub fn configurar() {
    let dir = match modelos::dir_modelos() {
        Some(d) => d,
        None => {
            log::aviso(
                "no se encuentra el directorio de modelos 3D: no se dibujara ningun modelo \
                 (pon XFACTORVR_MODELOS_DIR con la ruta)",
            );
            return;
        }
    };
    let manifiesto = match modelos::cargar_manifesto(&dir) {
        Some(m) => m,
        None => {
            log::aviso(&format!(
                "el manifiesto de modelos de {} no se puede leer: no habra 3D",
                dir.display()
            ));
            return;
        }
    };
    log::info(&format!(
        "modelos 3D: {} prototipos en {}",
        manifiesto.modelos.len(),
        dir.display()
    ));
    let mut r = registro();
    r.manifiesto = Some(manifiesto);
}

/// Indice de un prototipo en el registro (lo registra si no estaba).
pub fn indice_de_prototipo(nombre: &str) -> Option<usize> {
    let mut r = registro();
    if let Some(i) = r.prototipos.iter().position(|p| p == nombre) {
        return Some(i);
    }
    // Solo se registran los prototipos que tienen modelo.
    let tiene = r
        .manifiesto
        .as_ref()
        .map(|m| m.modelo(nombre).is_some())
        .unwrap_or(false);
    if !tiene {
        return None;
    }
    let i = r.prototipos.len();
    r.prototipos.push(nombre.to_string());
    r.cpu.push(None);
    r.gpu.push(None);
    Some(i)
}

/// Carga (en CPU) el modelo de un prototipo, si no estaba ya.
fn cargar_cpu(indice: usize) -> bool {
    let (nombre, dir, partes, huella) = {
        let r = registro();
        let nombre = match r.prototipos.get(indice) {
            Some(n) => n.clone(),
            None => return false,
        };
        let dir = match modelos::dir_modelos() {
            Some(d) => d,
            None => return false,
        };
        let partes: Vec<(String, Giro)> = r
            .manifiesto
            .as_ref()
            .and_then(|m| m.modelo(&nombre))
            .map(|i| i.partes.iter().map(|p| (p.archivo.clone(), p.giro)).collect())
            .unwrap_or_default();
        let huella = r
            .manifiesto
            .as_ref()
            .and_then(|m| m.modelo(&nombre))
            .map(|i| i.huella_tiles)
            .unwrap_or(1.0);
        (nombre, dir, partes, huella)
    };
    if partes.is_empty() {
        return false;
    }

    // Se lee el primer GLB (las partes se dibujan como una sola malla: para
    // el motor ARM64 se simplifica a la parte principal, que es la que tiene
    // la forma de la maquina).
    let mut combinado: Option<Modelo> = None;
    for (archivo, _giro) in partes.iter() {
        let ruta: PathBuf = dir.join(archivo);
        let bytes = match std::fs::read(&ruta) {
            Ok(b) => b,
            Err(e) => {
                log::aviso(&format!("no se pudo leer {}: {e}", ruta.display()));
                continue;
            }
        };
        match glb::cargar(&bytes) {
            Some(m) => {
                combinado = Some(m);
                break;
            }
            None => log::aviso(&format!("{} no es un GLB valido", ruta.display())),
        }
    }
    let mut modelo = match combinado {
        Some(m) => m,
        None => return false,
    };

    // Se recentra: el origen en el centro de la huella y la base en Y = 0.
    let cx = (modelo.bbox_min[0] + modelo.bbox_max[0]) * 0.5;
    let cz = (modelo.bbox_min[2] + modelo.bbox_max[2]) * 0.5;
    let cy = modelo.bbox_min[1];
    for malla in modelo.mallas.iter_mut() {
        let n = malla.vertices.len() / glb::FLOATS_POR_VERTICE;
        for v in 0..n {
            malla.vertices[v * glb::FLOATS_POR_VERTICE] -= cx;
            malla.vertices[v * glb::FLOATS_POR_VERTICE + 1] -= cy;
            malla.vertices[v * glb::FLOATS_POR_VERTICE + 2] -= cz;
        }
    }

    // Casillas por unidad: la huella del prototipo cubre el lado mayor del
    // modelo.
    let ancho = (modelo.bbox_max[0] - modelo.bbox_min[0]).max(1e-6);
    let fondo = (modelo.bbox_max[2] - modelo.bbox_min[2]).max(1e-6);
    let lado = ancho.max(fondo);
    let casillas_por_unidad = huella / lado;

    {
        let mut r = registro();
        r.cpu[indice] = Some(ModeloCpu {
            modelo,
            casillas_por_unidad,
        });
    }
    log::info(&format!(
        "modelo '{}' cargado: {} vertices, {} triangulos, {} casillas de huella",
        nombre,
        combinado_vertices(&r_modelo(indice)),
        combinado_triangulos(&r_modelo(indice)),
        huella
    ));
    true
}

/// Atajo para leer el modelo en CPU (ya cargado).
fn r_modelo(indice: usize) -> Modelo {
    let r = registro();
    r.cpu
        .get(indice)
        .and_then(|c| c.as_ref())
        .map(|c| c.modelo.clone())
        .unwrap_or_default()
}

/// Numero de vertices del modelo en CPU.
fn combinado_vertices(m: &Modelo) -> usize {
    m.vertices()
}

/// Numero de triangulos del modelo en CPU.
fn combinado_triangulos(m: &Modelo) -> usize {
    m.triangulos()
}

/// Prepara el programa y las funciones de GL (una vez, con contexto vivo).
pub fn preparar_gl() {
    if PROGRAMA.get().is_some() {
        return;
    }
    let gl = match construir_gl() {
        Some(g) => g,
        None => return,
    };
    let version = cadena_gl(unsafe { (gl.get_string)(GL_VERSION) });
    let es_es = version.contains("OpenGL ES");
    let renderer = cadena_gl(unsafe { (gl.get_string)(GL_RENDERER) });
    log::info(&format!(
        "GL de los modelos 3D: {} ({}) (OpenGL ES: {})",
        version, renderer, es_es
    ));
    let _ = GL.set(gl);

    let g = GL.get().unwrap();
    let vs_fuente = if es_es { VS_ES } else { VS_DESKTOP };
    let fs_fuente = if es_es { FS_ES } else { FS_DESKTOP };
    let vs = compilar(g, GL_VERTEX_SHADER, vs_fuente.as_bytes());
    let fs = compilar(g, GL_FRAGMENT_SHADER, fs_fuente.as_bytes());
    if vs == 0 || fs == 0 {
        let _ = PROGRAMA.set(None);
        return;
    }
    let prog = unsafe { (g.create_program)() };
    unsafe {
        (g.attach_shader)(prog, vs);
        (g.attach_shader)(prog, fs);
        (g.link_program)(prog);
    }
    let mut ok: i32 = 0;
    unsafe { (g.get_programiv)(prog, GL_LINK_STATUS, &mut ok) };
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut n: i32 = 0;
        unsafe { (g.get_program_info_log)(prog, 1024, &mut n, buf.as_mut_ptr()) };
        let n = (n.max(0) as usize).min(buf.len() - 1);
        log::error(&format!(
            "no se pudo enlazar el programa de los modelos 3D: {}",
            String::from_utf8_lossy(&buf[..n])
        ));
        let _ = PROGRAMA.set(None);
        return;
    }
    let loc_model = unsafe { (g.get_uniform_location)(prog, b"u_model\0".as_ptr() as *const i8) };
    let loc_viewport =
        unsafe { (g.get_uniform_location)(prog, b"u_viewport\0".as_ptr() as *const i8) };
    log::info("programa de los modelos 3D creado");
    let _ = PROGRAMA.set(Some(Programa {
        programa: prog,
        loc_model,
        loc_viewport,
    }));
}

/// Sube un modelo a la GPU (la primera vez que se dibuja).
fn subir_gpu(indice: usize) -> bool {
    let g = match GL.get() {
        Some(g) => g,
        None => return false,
    };
    let (modelo, casillas_por_unidad) = {
        let r = registro();
        match r.cpu.get(indice).and_then(|c| c.as_ref()) {
            Some(c) => (c.modelo.clone(), c.casillas_por_unidad),
            None => return false,
        }
    };
    let mut mallas = Vec::new();
    for malla in modelo.mallas.iter() {
        let mut vao: u32 = 0;
        let mut vbo: u32 = 0;
        let mut ebo: u32 = 0;
        unsafe {
            (g.gen_vertex_arrays)(1, &mut vao);
            (g.gen_buffers)(1, &mut vbo);
            (g.gen_buffers)(1, &mut ebo);
            (g.bind_vertex_array)(vao);
            (g.bind_buffer)(GL_ARRAY_BUFFER, vbo);
            (g.buffer_data)(
                GL_ARRAY_BUFFER,
                (malla.vertices.len() * 4) as isize,
                malla.vertices.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            let stride = (glb::FLOATS_POR_VERTICE * 4) as i32;
            (g.enable_vertex_attrib_array)(0);
            (g.vertex_attrib_pointer)(0, 3, GL_FLOAT, 0, stride, std::ptr::null());
            (g.enable_vertex_attrib_array)(1);
            (g.vertex_attrib_pointer)(
                1,
                3,
                GL_FLOAT,
                0,
                stride,
                (3 * 4) as *const c_void,
            );
            (g.enable_vertex_attrib_array)(2);
            (g.vertex_attrib_pointer)(
                2,
                4,
                GL_FLOAT,
                0,
                stride,
                (6 * 4) as *const c_void,
            );
            (g.bind_buffer)(GL_ELEMENT_ARRAY_BUFFER, ebo);
            (g.buffer_data)(
                GL_ELEMENT_ARRAY_BUFFER,
                (malla.indices.len() * 4) as isize,
                malla.indices.as_ptr() as *const c_void,
                GL_STATIC_DRAW,
            );
            (g.bind_vertex_array)(0);
        }
        mallas.push(MallaGpu {
            vao,
            indices: malla.indices.len(),
        });
    }
    let mut r = registro();
    if r.gpu.len() <= indice {
        r.gpu.resize(indice + 1, None);
    }
    r.gpu[indice] = Some(ModeloGpu { mallas });
    let _ = casillas_por_unidad;
    true
}

/// La matriz que lleva el modelo al framebuffer del mundo.
///
/// Calculada en la CPU (son cuatro multiplications por instancia).
fn matriz(inst: &Instancia, pixeles_por_unidad: f32) -> [f32; 16] {
    let yaw = YAW_BASE - (inst.direccion as f32) * (core::f32::consts::TAU / 16.0);
    let (sy, cy) = (yaw.sin(), yaw.cos());
    let (sp, cp) = (INCLINACION.sin(), INCLINACION.cos());
    let k = pixeles_por_unidad;

    // Columnas (column-major), como espera GL:
    //   eje X del modelo -> (cos yaw, -sin(p) sin yaw)
    //   eje Y (altura)   -> (0, -cos p)
    //   eje Z (fondo)    -> (sin yaw, sin(p) cos yaw)
    [
        cy * k,
        -sp * sy * k,
        0.0,
        0.0,
        0.0,
        -cp * k,
        0.0,
        0.0,
        sy * k,
        sp * cy * k,
        0.0,
        0.0,
        inst.x,
        inst.y,
        0.0,
        1.0,
    ]
}

/// Dibuja una lista de instancias en el framebuffer que hay enlazado.
///
/// `vp` es el viewport del framebuffer del mundo.
pub fn dibujar(instancias: &[Instancia], vp: &[i32; 4]) {
    if instancias.is_empty() {
        return;
    }
    let g = match GL.get() {
        Some(g) => g,
        None => return,
    };
    let prog = match PROGRAMA.get() {
        Some(Some(p)) => p,
        _ => return,
    };
    if vp[2] <= 0 || vp[3] <= 0 {
        return;
    }

    // Se guarda el estado de GL que se toca y se devuelve al motor tal cual.
    let prog_previo = gl_int(g, GL_CURRENT_PROGRAM);
    let vao_previo = gl_int(g, GL_VERTEX_ARRAY_BINDING);
    let vbo_previo = gl_int(g, GL_ARRAY_BUFFER_BINDING);
    let depth_previo = gl_int(g, GL_DEPTH_TEST);
    let blend_previo = gl_int(g, GL_BLEND);
    let cull_previo = gl_int(g, GL_CULL_FACE);
    let mut recorte = [0i32; 4];
    unsafe { (g.get_integerv)(GL_SCISSOR_BOX, recorte.as_mut_ptr()) };

    unsafe {
        (g.disable)(GL_DEPTH_TEST);
        (g.disable)(GL_CULL_FACE);
        (g.disable)(GL_BLEND);
        (g.blend_func_separate)(
            GL_SRC_ALPHA,
            GL_ONE_MINUS_SRC_ALPHA,
            GL_SRC_ALPHA,
            GL_ONE_MINUS_SRC_ALPHA,
        );
        (g.use_program)(prog.programa);
        (g.uniform_2f)(
            prog.loc_viewport,
            vp[2] as f32,
            vp[3] as f32,
        );
    }

    let mut dibujados = 0u64;
    for inst in instancias.iter() {
        // El modelo tiene que estar en CPU y en GPU.
        {
            let r = registro();
            if r.cpu.get(inst.modelo).map(|c| c.is_none()).unwrap_or(true) {
                drop(r);
                if !cargar_cpu(inst.modelo) {
                    continue;
                }
            }
        }
        {
            let r = registro();
            if r.gpu.get(inst.modelo).map(|c| c.is_none()).unwrap_or(true) {
                drop(r);
                if !subir_gpu(inst.modelo) {
                    continue;
                }
            }
        }
        let (mallas, casillas_por_unidad) = {
            let r = registro();
            let gpu = match r.gpu.get(inst.modelo).and_then(|g| g.as_ref()) {
                Some(g) => g,
                None => continue,
            };
            let cpu = match r.cpu.get(inst.modelo).and_then(|c| c.as_ref()) {
                Some(c) => c,
                None => continue,
            };
            (gpu.mallas.clone(), cpu.casillas_por_unidad)
        };
        let pixeles_por_unidad = casillas_por_unidad * inst.pixeles_por_tile;
        let m = matriz(inst, pixeles_por_unidad);

        unsafe {
            // Recorte a todo el viewport: el motor dibuja el mundo por trozos.
            (g.scissor)(0, 0, vp[2], vp[3]);
            (g.uniform_matrix4fv)(prog.loc_model, 1, 0, m.as_ptr());
        }
        for malla in mallas.iter() {
            unsafe {
                (g.bind_vertex_array)(malla.vao);
                (g.draw_elements)(
                    GL_TRIANGLES,
                    malla.indices as i32,
                    GL_UNSIGNED_INT,
                    std::ptr::null(),
                );
            }
            dibujados += 1;
        }
    }
    DIBUJADOS.fetch_add(dibujados, Ordering::Relaxed);
    // Los errores de GL son pegajosos: se limpian y se avisa si hay alguno.
    loop {
        let e = unsafe { (g.get_error)() };
        if e == 0 {
            break;
        }
        log::aviso(&format!("error de GL 0x{e:x} al dibujar los modelos 3D"));
    }

    // Se devuelve el estado al motor.
    unsafe {
        (g.scissor)(recorte[0], recorte[1], recorte[2], recorte[3]);
        (g.bind_vertex_array)(vao_previo as u32);
        (g.bind_buffer)(GL_ARRAY_BUFFER, vbo_previo as u32);
        if depth_previo != 0 {
            (g.enable)(GL_DEPTH_TEST);
        }
        if blend_previo != 0 {
            (g.enable)(GL_BLEND);
        }
        if cull_previo != 0 {
            (g.enable)(GL_CULL_FACE);
        }
        (g.use_program)(prog_previo as u32);
    }
}

/// Numero de mallas dibujadas (para el log).
pub static DIBUJADOS: AtomicU64 = AtomicU64::new(0);

/// Numero de mallas de modelo dibujadas.
pub fn dibujados() -> u64 {
    DIBUJADOS.load(Ordering::Relaxed)
}

/// Compila un shader.
fn compilar(g: &Gl, tipo: u32, fuente: &[u8]) -> u32 {
    let id = unsafe { (g.create_shader)(tipo) };
    if id == 0 {
        return 0;
    }
    let ptr = fuente.as_ptr() as *const i8;
    let len = fuente.len() as i32;
    unsafe {
        (g.shader_source)(id, 1, &ptr, &len);
        (g.compile_shader)(id);
    }
    let mut ok: i32 = 0;
    unsafe { (g.get_shaderiv)(id, GL_COMPILE_STATUS, &mut ok) };
    if ok == 0 {
        let mut buf = [0u8; 1024];
        let mut n: i32 = 0;
        unsafe { (g.get_shader_info_log)(id, 1024, &mut n, buf.as_mut_ptr()) };
        let n = (n.max(0) as usize).min(buf.len() - 1);
        log::error(&format!(
            "error al compilar el shader de los modelos 3D: {}",
            String::from_utf8_lossy(&buf[..n])
        ));
        return 0;
    }
    id
}

/// Lee un entero de GL.
fn gl_int(g: &Gl, pname: u32) -> i32 {
    let mut v: i32 = 0;
    unsafe { (g.get_integerv)(pname, &mut v) };
    v
}

/// Convierte una cadena de GL en `String`.
fn cadena_gl(p: *const u8) -> String {
    if p.is_null() {
        return "<null>".to_string();
    }
    let mut n = 0usize;
    unsafe {
        while *p.add(n) != 0 {
            n += 1;
        }
        String::from_utf8_lossy(std::slice::from_raw_parts(p, n)).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn la_matriz_coloca_el_modelo_en_pixels() {
        let inst = Instancia {
            modelo: 0,
            x: 100.0,
            y: 200.0,
            direccion: 0,
            pixeles_por_tile: 32.0,
        };
        let m = matriz(&inst, 1.0);
        // La traslacion es el punto donde se apoya la maquina.
        assert_eq!(m[12], 100.0);
        assert_eq!(m[13], 200.0);
        // La altura del modelo sube en la pantalla (Y hacia abajo): la columna
        // Y tiene la Y negativa.
        assert!(m[5] < 0.0, "la altura deberia subir en pantalla");
        // Y la anchura no se inclina (con el giro base de 180 grados, el eje X
        // del modelo queda en -X de la pantalla: el modelo mira al norte).
        assert!((m[0] + 1.0).abs() < 1e-5);
    }

    #[test]
    fn la_direccion_gira_el_modelo() {
        let mut a = Instancia {
            modelo: 0,
            x: 0.0,
            y: 0.0,
            direccion: 0,
            pixeles_por_tile: 32.0,
        };
        let m0 = matriz(&a, 1.0);
        a.direccion = 4; // 90 grados
        let m4 = matriz(&a, 1.0);
        // Con direccion 0 el frente (eje Z) va hacia el norte (arriba).
        assert!(m0[9] < 0.0, "el frente deberia mirar al norte");
        // Con direccion 4 el frente va al este (derecha).
        assert!(m4[8] > 0.9, "el frente deberia mirar al este");
    }
}

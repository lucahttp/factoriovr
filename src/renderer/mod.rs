//! renderer — El renderizador 3D del VISOR.
//!
//! Aqui se dibujan las maquinas de Factorio con su MODELO 3D REAL (los GLB
//! del mod `factorio-3d-mod`), instanciadas: una matriz por entidad y una
//! llamada de dibujo por prototipo.  Las entidades que NO tienen modelo
//! (items sueltos, arbustos...) las sigue pintando el motor de VR con el
//! sprite 2D del atlas: nunca queda un hueco.
//!
//! ## Como se dibuja una entidad con su modelo
//!
//! 1. El puente de Factorio manda el prototipo de cada entidad viva; el
//!    modulo de modelos (`src/modelos3d.rs`) le asigna un `model_id`
//!    estable (0 = sin modelo).
//! 2. Aqui se agrupan las entidades por prototipo y se calcula una matriz
//!    por entidad: `T(casilla) * Ry(giro) * S(escala) * T(-offset)`.
//! 3. La malla (los vertices del GLB, con el color ya horneado) se sube a
//!    la GPU una sola vez y se dibuja con `glDrawArraysInstanced`.
//!
//! ## Lo que hace el motor de VR (C++)
//!
//! El motor de VR es el duenno del contexto GL y del framebuffer, asi que
//! el C++ llama a estas funciones (ver `android/src/openxr_engine.cpp`):
//!
//! ```c
//! uint32_t xfactorvr_3d_iniciar();                       // una vez, al arrancar
//! void    xfactorvr_3d_fijar_camaras(const float* vp, const float* tabla);
//! uint32_t xfactorvr_dibujar_modelos_3d();               // por ojo y frame
//! bool    xfactorvr_3d_tiene_modelo(uint32_t model_id);  // para no pintar el sprite
//! ```
//!
//! `vp` es la matriz vista-proyeccion (16 floats, column-major) y `tabla` la
//! matriz de la mesa (la misma que usa el motor para los sprites, que
//! coloca la maqueta en la mesa fisica de la habitacion).

pub mod gl;
pub mod model;
pub mod sgsr;
pub mod shader;

use crate::modelos3d;
use glam::Mat4;
use gl::{Gl, GLboolean};
use model::{Ajustes, ModelRenderer};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock, RwLock};

// ---------------------------------------------------------------------------
// GEOMETRIA QUE DIBUJA EL MOTOR (canal del ADR-021)
// ---------------------------------------------------------------------------

/// Una entidad tal como la dibuja el motor.
#[derive(Clone, Copy, Debug, Default)]
pub struct GeoEntidad {
    /// Posicion en el mundo del motor (tiles).
    pub wx: f32,
    pub wy: f32,
    /// Tamano del sprite (tiles).
    pub sx: f32,
    pub sy: f32,
    /// Direccion (0..15): 0 = norte, 4 = este, 8 = sur, 12 = oeste.
    pub direccion: u32,
    /// Indice en la tabla de nombres de prototipo.
    pub modelo: u32,
}

static GEOMETRIA: OnceLock<RwLock<Vec<GeoEntidad>>> = OnceLock::new();
static GEOMETRIA_NOMBRES: OnceLock<RwLock<Vec<String>>> = OnceLock::new();

fn geometria_lock() -> &'static RwLock<Vec<GeoEntidad>> {
    GEOMETRIA.get_or_init(|| RwLock::new(Vec::new()))
}

fn geometria_nombres_lock() -> &'static RwLock<Vec<String>> {
    GEOMETRIA_NOMBRES.get_or_init(|| RwLock::new(Vec::new()))
}

/// Las entidades que el motor ha dibujado (vacio si el canal no manda nada).
pub fn geometria() -> Vec<GeoEntidad> {
    geometria_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default()
}

/// Recibe la geometria que dibuja el motor.
///
/// La llama el visor, que la recibe del inyector por el socket del render
/// (canal del ADR-021).  `datos` son 6 floats por entidad (wx, wy, sx, sy,
/// direccion, modelo) y `nombres` la tabla de prototipos, separada por '\\n'.
///
/// Devuelve cuantas entidades se han guardado.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_geometria(
    datos: *const f32,
    n: u32,
    nombres: *const std::ffi::c_char,
) -> u32 {
    if datos.is_null() || n == 0 {
        return 0;
    }
    let mut ents = Vec::with_capacity(n as usize);
    for i in 0..n as usize {
        // SAFETY: el visor manda n*6 floats en un buffer valido.
        let p = unsafe { datos.add(i * 6) };
        ents.push(GeoEntidad {
            wx: unsafe { *p },
            wy: unsafe { *p.add(1) },
            sx: unsafe { *p.add(2) },
            sy: unsafe { *p.add(3) },
            direccion: unsafe { *p.add(4) } as u32,
            modelo: unsafe { *p.add(5) } as u32,
        });
    }
    let tabla: Vec<String> = if nombres.is_null() {
        Vec::new()
    } else {
        // SAFETY: el visor manda una cadena terminada en NUL.
        unsafe { std::ffi::CStr::from_ptr(nombres) }
            .to_string_lossy()
            .split('\n')
            .map(|s| s.to_string())
            .collect()
    };
    *geometria_nombres_lock().write().unwrap_or_else(|e| e.into_inner()) = tabla;
    let total = ents.len() as u32;
    *geometria_lock().write().unwrap_or_else(|e| e.into_inner()) = ents;
    log::info!(
        "geometria del motor: {} entidades (canal XFV1)",
        total
    );
    total
}

/// Estado global del renderer (el motor de VR es mono-hilo en el render).
struct Estado {
    gl: Option<Gl>,
    modelos: Option<ModelRenderer>,
    sgsr: Option<sgsr::SgsrUpscaler>,
    /// Vista-proyeccion de la ultima llamada.
    vp: Mat4,
    /// Matriz de la mesa (maqueta -> habitacion).
    tabla: Mat4,
    /// Posicion de la camara en la habitacion (para el especular).
    ojos_habitacion: [f32; 3],
    ajustes: Ajustes,
    /// Instancias dibujadas en el ultimo frame.
    ultimo: u32,
}

impl Estado {
    fn nuevo() -> Self {
        Self {
            gl: None,
            modelos: None,
            sgsr: None,
            vp: Mat4::IDENTITY,
            tabla: Mat4::IDENTITY,
            ojos_habitacion: [0.0, 0.4, 0.0],
            ajustes: Ajustes::default(),
            ultimo: 0,
        }
    }
}

fn estado() -> &'static Mutex<Estado> {
    static ESTADO: OnceLock<Mutex<Estado>> = OnceLock::new();
    ESTADO.get_or_init(|| Mutex::new(Estado::nuevo()))
}

// ---------------------------------------------------------------------------
// API publica (Rust)
// ---------------------------------------------------------------------------

/// Prepara el renderer: carga el contexto GL y compila los shaders.
///
/// Es idempotente: llamarlo otra vez no hace nada.  Devuelve false si no hay
/// GL (por ejemplo, en el host de los tests).
pub fn iniciar() -> bool {
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    if e.modelos.is_some() {
        return true;
    }
    let gl = match gl::cargar() {
        Ok(g) => g,
        Err(err) => {
            log::warn!("[renderer] no hay OpenGL ES: {err:#}");
            return false;
        }
    };
    match ModelRenderer::new(&gl) {
        Ok(m) => {
            e.gl = Some(gl);
            e.modelos = Some(m);
            log::info!("[renderer] renderer 3D listo");
            true
        }
        Err(err) => {
            log::error!("[renderer] no se pudo crear el renderer 3D: {err:#}");
            false
        }
    }
}

/// Fija la vista-proyeccion y la matriz de la mesa para el proximo frame.
///
/// - `vp`: vista-proyeccion (16 floats, column-major).
/// - `tabla`: matriz de la mesa (16 floats, column-major): coloca la
///   maqueta (en metros) sobre la mesa fisica de la habitacion.
pub fn fijar_camaras(vp: Mat4, tabla: Mat4) {
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.vp = vp;
    e.tabla = tabla;
}

/// Fija la posicion de la camara en el espacio de la HABITACION (3 floats).
pub fn fijar_ojos(ojos: [f32; 3]) {
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.ojos_habitacion = ojos;
}

/// Ajustes de luz y recorte.
pub fn ajustes() -> Ajustes {
    estado()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .ajustes
}

/// Cambia los ajustes de luz y recorte.
pub fn fijar_ajustes(a: Ajustes) {
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.ajustes = a;
}

/// Numero de instancias (entidades) dibujadas en el ultimo frame.
pub fn ultimo_contador() -> u32 {
    estado()
        .lock()
        .unwrap_or_else(|err| err.into_inner())
        .ultimo
}

/// Dibuja TODAS las entidades vivas que tienen modelo 3D.
///
/// Usa la vista-proyeccion y la matriz de la mesa fijadas con
/// `fijar_camaras`.  Devuelve el numero de instancias dibujadas.
pub fn dibujar_entidades_3d() -> u32 {
    let (gl, vp, tabla) = {
        let e = estado().lock().unwrap_or_else(|err| err.into_inner());
        match e.gl {
            Some(g) => (g, e.vp, e.tabla),
            None => return 0,
        }
    };
    dibujar_con(&gl, &vp, &tabla)
}

/// Dibuja las entidades con modelo 3D con las matrices dadas.
///
/// `vp` es vista-proyeccion y `tabla` la matriz de la mesa.
pub fn dibujar_con(gl: &Gl, vp: &Mat4, tabla: &Mat4) -> u32 {
    let (ajustes, ojos) = {
        let e = estado().lock().unwrap_or_else(|err| err.into_inner());
        (e.ajustes, e.ojos_habitacion)
    };

    // 1. Que instancias hay: una matriz por entidad y por parte del modelo.
    let grupos = recopilar_instancias();

    // 2. Subir las mallas que falten y dibujar.
    let mut total = 0u32;
    let mut modelos = {
        let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
        match e.modelos.take() {
            Some(m) => m,
            None => return 0,
        }
    };

    // Estado de GL: las maquinas son opacas, con prueba de profundidad.
    // Se guarda el estado del motor de VR y se devuelve tal cual.
    let (blend, depth, cull, depth_mask) = unsafe {
        (
            (gl.IsEnabled)(gl::BLEND),
            (gl.IsEnabled)(gl::DEPTH_TEST),
            (gl.IsEnabled)(gl::CULL_FACE),
            {
                let mut m: GLboolean = 0;
                (gl.GetBooleanv)(gl::DEPTH_WRITEMASK, &mut m);
                m
            },
        )
    };
    unsafe {
        (gl.Enable)(gl::DEPTH_TEST);
        (gl.DepthFunc)(gl::LEQUAL);
        (gl.DepthMask)(gl::TRUE_);
        (gl.Disable)(gl::BLEND);
        (gl.Disable)(gl::CULL_FACE);
    }

    let mvp = (*vp) * (*tabla);
    let mut ajustes = ajustes;
    ajustes.ojos = model::ojos_en_mesa(tabla, ojos);

    for ((_proto, archivo), matrices) in &grupos {
        // La malla se sube la primera vez que aparece.
        if let Some(geo) = modelos3d::geometria_de_archivo(archivo) {
            if let Err(err) = modelos.subir_malla(gl, archivo, &geo.vertices, &geo.indices) {
                log::warn!("[renderer] no se pudo subir {archivo}: {err:#}");
                continue;
            }
        }
        total += modelos.dibujar_instancias(gl, archivo, matrices, &mvp, &ajustes);
    }

    // Se deja el estado como lo encontro el motor.
    unsafe {
        (gl.UseProgram)(0);
        (gl.BindVertexArray)(0);
        if blend != 0 {
            (gl.Enable)(gl::BLEND);
        } else {
            (gl.Disable)(gl::BLEND);
        }
        if depth != 0 {
            (gl.Enable)(gl::DEPTH_TEST);
        } else {
            (gl.Disable)(gl::DEPTH_TEST);
        }
        if cull != 0 {
            (gl.Enable)(gl::CULL_FACE);
        } else {
            (gl.Disable)(gl::CULL_FACE);
        }
        (gl.DepthMask)(depth_mask);
    }

    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.modelos = Some(modelos);
    e.ultimo = total;
    total
}

/// Dibuja las entidades con modelo 3D (nombre corto, el que usa la FFI).
pub fn dibujar_entidades_3d_con(gl: &Gl, vp: &Mat4, tabla: &Mat4) -> u32 {
    dibujar_con(gl, vp, tabla)
}

/// Dice si una entidad tiene modelo 3D (y por tanto NO hay que pintarle
/// el sprite 2D).
pub fn tiene_modelo(model_id: u32) -> bool {
    model_id != 0 && modelos3d::prototipo_de_id(model_id).is_some()
}

/// Agrupa las entidades vivas por (prototipo, parte) y calcula su matriz.
///
/// Es la parte "de CPU" del render: no necesita GL, asi que se puede
/// comprobar en los tests.
pub fn recopilar_instancias() -> HashMap<(String, String), Vec<Mat4>> {
    // Si el motor manda geometria (canal del ADR-021), manda ella: sus
    // posiciones y direcciones son las EXACTAS con las que dibuja, y el
    // snapshot solo las estima.
    let geo = geometria();
    if !geo.is_empty() {
        return recopilar_instancias_de_geometria(&geo);
    }

    let mut grupos: HashMap<(String, String), Vec<Mat4>> = HashMap::new();
    let escala = modelos3d::escala_tile();
    let (pan_x, pan_y) = crate::game_bridge::get_simulation_world()
        .read()
        .map(|w| w.view_center)
        .unwrap_or((0.0, 0.0));
    let entidades = crate::get_live_entities_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();

    for e in entidades {
        if e.model_id == 0 {
            continue;
        }
        let Some(proto) = modelos3d::prototipo_de_id(e.model_id) else {
            continue;
        };
        let Some(modelo) = modelos3d::modelo_instanciable(&proto) else {
            continue;
        };
        for parte in &modelo.partes {
            let m = modelos3d::matriz_instancia(
                &modelo,
                parte,
                e.tile_x - pan_x,
                e.tile_y - pan_y,
                e.orientation,
                e.anim_frame,
                escala,
            );
            grupos
                .entry((proto.clone(), parte.archivo.clone()))
                .or_default()
                .push(m);
        }
    }
    grupos
}

/// Las instancias a partir de la geometria que dibuja el motor.
///
/// Es el camino "bueno": las posiciones y las direcciones son las que el
/// motor ha dibujado de verdad (canal del ADR-021), no las estimadas por el
/// snapshot.  El prototipo se saca de la tabla de nombres que llega con el
/// mensaje, y el yaw de las 16 direcciones del motor (0 = norte).
fn recopilar_instancias_de_geometria(
    geo: &[GeoEntidad],
) -> HashMap<(String, String), Vec<Mat4>> {
    let mut grupos: HashMap<(String, String), Vec<Mat4>> = HashMap::new();
    let escala = modelos3d::escala_tile();
    let (pan_x, pan_y) = crate::game_bridge::get_simulation_world()
        .read()
        .map(|w| w.view_center)
        .unwrap_or((0.0, 0.0));
    let nombres = geometria_nombres_lock()
        .read()
        .map(|v| v.clone())
        .unwrap_or_default();

    for e in geo {
        let Some(proto) = nombres.get(e.modelo as usize) else {
            continue;
        };
        let Some(modelo) = modelos3d::modelo_instanciable(proto) else {
            continue;
        };
        // Factorio 2.0 usa 16 direcciones; los radianes son dir * TAU/16.
        let yaw = (e.direccion as f32) * (std::f32::consts::TAU / 16.0);
        for parte in &modelo.partes {
            let m = modelos3d::matriz_instancia(
                &modelo,
                parte,
                e.wx - pan_x,
                e.wy - pan_y,
                yaw,
                0,
                escala,
            );
            grupos
                .entry((proto.clone(), parte.archivo.clone()))
                .or_default()
                .push(m);
        }
    }
    grupos
}

// ---------------------------------------------------------------------------
// SGSR
// ---------------------------------------------------------------------------

/// Prepara el upscaling SGSR: entrada a `input` y salida a `output`.
///
/// El motor de VR dibuja la escena en el framebuffer que devuelve
/// `xfactorvr_sgsr_fbo()` y luego llama a `xfactorvr_sgsr_escalar()`.
pub fn sgsr_preparar(input: (u32, u32), output: (u32, u32)) -> bool {
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    let Some(gl) = e.gl else {
        return false;
    };
    match sgsr::SgsrUpscaler::new(&gl, input, output) {
        Ok(s) => {
            e.sgsr = Some(s);
            true
        }
        Err(err) => {
            log::error!("[renderer] no se pudo preparar SGSR: {err:#}");
            false
        }
    }
}

/// Nombre GL del framebuffer de entrada de SGSR (0 si no esta preparado).
pub fn sgsr_framebuffer() -> u32 {
    let e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.sgsr.as_ref().map(|s| s.framebuffer()).unwrap_or(0)
}

/// Nombre GL de la textura de entrada de SGSR (0 si no esta preparado).
pub fn sgsr_textura() -> u32 {
    let e = estado().lock().unwrap_or_else(|err| err.into_inner());
    e.sgsr.as_ref().map(|s| s.textura()).unwrap_or(0)
}

/// Reescala la entrada de SGSR al framebuffer que este ligado ahora.
pub fn sgsr_escalar() -> bool {
    let e = estado().lock().unwrap_or_else(|err| err.into_inner());
    let Some(gl) = e.gl else {
        return false;
    };
    let Some(s) = e.sgsr.as_ref() else {
        return false;
    };
    s.escalar(&gl)
}

/// Lee una matriz 4x4 (16 floats, column-major) de un puntero de C.
unsafe fn matriz16(p: *const f32) -> [f32; 16] {
    std::slice::from_raw_parts(p, 16).try_into().unwrap_or([0.0; 16])
}

// ---------------------------------------------------------------------------
// FFI para el motor de VR (C++)
// ---------------------------------------------------------------------------

/// Prepara el renderer 3D. Devuelve true si esta listo.
///
/// Llamar UNA vez al arrancar el motor (con el contexto GL ya corriente).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_3d_iniciar() -> bool {
    iniciar()
}

/// Fija la vista-proyeccion y la matriz de la mesa.
///
/// - `vp`: 16 floats (column-major), vista-proyeccion.
/// - `tabla`: 16 floats (column-major), matriz de la mesa.
///
/// Si alguno de los dos punteros es NULL se ignora ese parametro.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_3d_fijar_camaras(vp: *const f32, tabla: *const f32) -> bool {
    if vp.is_null() && tabla.is_null() {
        return false;
    }
    let mut e = estado().lock().unwrap_or_else(|err| err.into_inner());
    if !vp.is_null() {
        e.vp = Mat4::from_cols_array(&matriz16(vp));
    }
    if !tabla.is_null() {
        e.tabla = Mat4::from_cols_array(&matriz16(tabla));
    }
    true
}

/// Fija la posicion de la camara en el espacio de la HABITACION (3 floats).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_3d_fijar_ojos(ojos: *const f32) -> bool {
    if ojos.is_null() {
        return false;
    }
    let v = std::slice::from_raw_parts(ojos, 3);
    fijar_ojos([v[0], v[1], v[2]]);
    true
}

/// Dibuja las entidades con MODELO 3D.
///
/// Devuelve el numero de instancias dibujadas (0 si no hay nada que
/// dibujar o si el renderer no esta preparado).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_dibujar_modelos_3d() -> u32 {
    dibujar_entidades_3d()
}

/// Igual que `xfactorvr_dibujar_modelos_3d`, pero pasando las matrices en
/// la propia llamada (16 floats cada una).
#[unsafe(no_mangle)]
pub unsafe extern "C" fn xfactorvr_dibujar_modelos_3d_ex(vp: *const f32, tabla: *const f32) -> u32 {
    if vp.is_null() || tabla.is_null() {
        return 0;
    }
    let vp = Mat4::from_cols_array(&matriz16(vp));
    let tabla = Mat4::from_cols_array(&matriz16(tabla));
    let gl = { estado().lock().unwrap_or_else(|err| err.into_inner()).gl };
    let Some(gl) = gl else {
        return 0;
    };
    dibujar_con(&gl, &vp, &tabla)
}

/// Dice si esta entidad se dibuja con un modelo 3D (y por tanto el motor NO
/// tiene que pintarle el sprite 2D del atlas).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_3d_tiene_modelo(model_id: u32) -> bool {
    tiene_modelo(model_id)
}

/// Numero de instancias dibujadas en el ultimo frame.
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_3d_ultimo_contador() -> u32 {
    ultimo_contador()
}

/// Prepara el upscaling SGSR (entrada y salida, en pixeles).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_sgsr_preparar(
    ancho_entrada: i32,
    alto_entrada: i32,
    ancho_salida: i32,
    alto_salida: i32,
) -> bool {
    if ancho_entrada <= 0 || alto_entrada <= 0 || ancho_salida <= 0 || alto_salida <= 0 {
        return false;
    }
    sgsr_preparar(
        (ancho_entrada as u32, alto_entrada as u32),
        (ancho_salida as u32, alto_salida as u32),
    )
}

/// Nombre GL del framebuffer donde hay que renderizar la escena a baja
/// resolucion (0 si SGSR no esta preparado).
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_sgsr_fbo() -> u32 {
    sgsr_framebuffer()
}

/// Nombre GL de la textura de entrada de SGSR.
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_sgsr_textura() -> u32 {
    sgsr_textura()
}

/// Reescala la entrada de SGSR al framebuffer ligado ahora mismo.
#[unsafe(no_mangle)]
pub extern "C" fn xfactorvr_sgsr_escalar() -> bool {
    sgsr_escalar()
}

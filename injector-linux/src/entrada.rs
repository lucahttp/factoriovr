//! entrada.rs — Inyectar la entrada del visor VR **dentro del motor**, como si
//! fuera el ratón del jugador.
//!
//! # Por que
//!
//! La forma "bonita" de jugar en VR seria mandar comandos (construir, minar,
//! investigar). Pero un comando es una **decision del juego**, y si la decision
//! la toma el puente (RCON o el mod), el motor deja de ser el juego: el fantasma
//! de construccion no aparece, las rotaciones no encajan, el fantasma de
//! investigacion no se ve, el "no puedes construir aqui" no sale, y el
//! deshacer no funciona.
//!
//! En vez de eso, aqui se hace lo que hace el mod 3D de GDani31 en Windows:
//! el motor recibe **un raton**, y el motor decide. La cadena es:
//!
//! ```text
//!   mano del VR -> rayo -> pixel del frame del motor -> inyector -> SDL
//!                 (el motor lo convierte en posicion de mapa: GameView::getMapPosition)
//! ```
//!
//! Por eso **no hace falta RCON para las ordenes**: el clic entra por la misma
//! puerta que el raton de un jugador.
//!
//! # Por que el pixel se puede calcular en el visor
//!
//! El visor presenta el frame del motor como un **plano en la mesa**. Un rayo
//! que golpea ese plano da unas coordenadas de textura (u, v) que son
//! exactamente un pixel de la imagen del motor. No hay que saber nada de la
//! camara: el rayo ES el raton.
//!
//! Las coordenadas llegan en el espacio de la imagen **transmitida** (que puede
//! estar reescalada), asi que aqui se pasan a las del framebuffer real.
//!
//! # Protocolo "XFCE"
//!
//! TCP, little-endian. El **inyector es el servidor** (a diferencia del canal
//! de frames, donde el inyector es el cliente), para que el canal de entrada
//! no compita con el de imagen y para poder reconectar sin que el juego se
//! entere.
//!
//! ```text
//!   cabecera (7 bytes)
//!     0..4    magic   u32 = 0x45434658  ("XFCE")
//!     4       tipo    u8   (1 = clic)
//!     5..7    tamano  u16  (bytes de la carga)
//!   carga del clic (6 bytes)
//!     0..2    x       u16  (pixel en la imagen transmitida)
//!     2..4    y       u16
//!     4       boton   u8   (1 = izquierdo, 2 = medio, 3 = derecho)
//!     5       accion  u8   (1 = abajo, 2 = arriba, 3 = clic completo)
//! ```
//!
//! # Variables de entorno
//!
//! * `XFACTORVR_ENTRADA_PUERTO` — puerto TCP donde escuchar. Si no esta, el
//!   modulo no hace nada.

use std::io::Read;
use std::net::TcpListener;
use std::net::TcpStream;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::OnceLock;

use crate::log;
use crate::resolver::Simbolos;

/// "XFCE" en little-endian.
pub const MAGIC: u32 = 0x4543_4658;
/// Bytes de la cabecera de un mensaje: magic (4) + tipo (1) + tamano (2).
pub const CABECERA: usize = 7;
/// Tipo de mensaje: clic del raton.
pub const TIPO_CLIC: u8 = 1;
/// Tipo de mensaje: tecla pulsada o soltada.
pub const TIPO_TECLA: u8 = 2;
/// Tipo de mensaje: rueda del raton (zoom).
pub const TIPO_RUEDA: u8 = 3;
/// Tipo de mensaje: movimiento del raton.
pub const TIPO_MOVIMIENTO: u8 = 4;

/// Accion del raton o tecla.
pub const ACCION_BAJO: u8 = 1;
pub const ACCION_ARRIBA: u8 = 2;
pub const ACCION_CLIC: u8 = 3;

/// Botones de SDL2.
pub const BOTON_IZQUIERDO: u8 = 1;
pub const BOTON_MEDIO: u8 = 2;
pub const BOTON_DERECHO: u8 = 3;

/// Tipo de evento SDL2 de boton de raton.
pub const SDL_MOUSEBUTTONDOWN: u32 = 0x0403;
/// Tipo de evento SDL2 de boton de raton (soltar).
pub const SDL_MOUSEBUTTONUP: u32 = 0x0404;
/// Tipo de evento SDL2 de teclado.
pub const SDL_KEYDOWN: u32 = 0x0300;
pub const SDL_KEYUP: u32 = 0x0301;
/// Tipo de evento SDL2 de rueda del raton.
pub const SDL_MOUSEWHEEL: u32 = 0x0405;

/// Boton o tecla pulsada.
pub const SDL_PRESSED: u8 = 1;
/// Boton o tecla soltada.
pub const SDL_RELEASED: u8 = 0;

/// `SDL_Event` es una union; se reserva de mas por si cambia el miembro mayor.
pub const TAM_EVENTO: usize = 64;

/// Un clic que llega del visor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clic {
    /// Pixel X en la imagen transmitida.
    pub x: u16,
    /// Pixel Y en la imagen transmitida.
    pub y: u16,
    /// Boton (1 = izquierdo, 2 = medio, 3 = derecho).
    pub boton: u8,
    /// Accion (1 = abajo, 2 = arriba, 3 = clic completo).
    pub accion: u8,
}

/// Un evento de teclado que llega del visor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tecla {
    /// Scancode de SDL2 (p.ej. 26 = W, 4 = A, 22 = S, 7 = D, 8 = E, 20 = Q, 21 = R, 41 = ESC).
    pub scancode: u16,
    /// Accion (1 = abajo, 2 = arriba).
    pub accion: u8,
}

/// Movimiento de la rueda del raton (zoom).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rueda {
    pub x: i16,
    pub y: i16,
}

/// Movimiento del raton (sin clic).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Movimiento {
    pub x: u16,
    pub y: u16,
}

/// Funciones de SDL que usa el motor, resueltas por nombre.
struct Sdl {
    /// `SDL_SendMouseMotion(SDL_Window*, uint32_t which, int relative, int x, int y)`.
    send_mouse_motion: unsafe extern "C" fn(*mut std::ffi::c_void, u32, i32, i32, i32) -> i32,
    /// `SDL_GetMouseFocus() -> SDL_Window*`.
    get_mouse_focus: unsafe extern "C" fn() -> *mut std::ffi::c_void,
    /// `SDL_GetWindowID(SDL_Window*) -> Uint32`.
    get_window_id: unsafe extern "C" fn(*mut std::ffi::c_void) -> u32,
    /// `SDL_GetWindowFromID(Uint32) -> SDL_Window*`.
    get_window_from_id: unsafe extern "C" fn(u32) -> *mut std::ffi::c_void,
    /// `SDL_GetWindowSize(SDL_Window*, int* w, int* h)`.
    get_window_size: unsafe extern "C" fn(*mut std::ffi::c_void, *mut i32, *mut i32),
    /// `SDL_PushEvent(SDL_Event*) -> int`.
    push_event: unsafe extern "C" fn(*const std::ffi::c_void) -> i32,
}

static SDL: OnceLock<Sdl> = OnceLock::new();

/// Ancho del framebuffer del mundo (para escalar los clics).
static VIEWPORT_W: AtomicU32 = AtomicU32::new(0);
/// Alto del framebuffer del mundo.
static VIEWPORT_H: AtomicU32 = AtomicU32::new(0);

/// Numero de clics inyectados (para el log y los bancos).
static CLICS: AtomicU32 = AtomicU32::new(0);

/// El modulo esta escuchando.
static ACTIVO: AtomicU32 = AtomicU32::new(0);

/// ¿Esta el puente de entrada activo?
pub fn activo() -> bool {
    ACTIVO.load(Ordering::Relaxed) != 0
}

/// Numero de clics inyectados desde que arranco el motor.
pub fn clics_inyectados() -> u32 {
    CLICS.load(Ordering::Relaxed)
}

/// Publica el tamano del framebuffer del mundo, para poder escalar los clics.
///
/// Lo llama la captura del frame (`modelo3d.rs`): el visor manda pixeles de la
/// imagen **transmitida**, que puede estar reescalada respecto al framebuffer.
pub fn fijar_viewport(ancho: u32, alto: u32) {
    VIEWPORT_W.store(ancho, Ordering::Relaxed);
    VIEWPORT_H.store(alto, Ordering::Relaxed);
}

/// Escala unas coordenadas de la imagen transmitida al framebuffer real.
///
/// `origen` es el tamano de la imagen que ve el visor y `destino` el del
/// framebuffer del motor. Si falta algun tamano, no se escala.
pub fn escalar(x: u16, y: u16, origen: (u32, u32), destino: (u32, u32)) -> (i32, i32) {
    let (ow, oh) = origen;
    let (dw, dh) = destino;
    if ow == 0 || oh == 0 || dw == 0 || dh == 0 {
        return (x as i32, y as i32);
    }
    let nx = (x as u64 * dw as u64 + (ow as u64 / 2)) / ow as u64;
    let ny = (y as u64 * dh as u64 + (oh as u64 / 2)) / oh as u64;
    let nx = nx.min(dw.saturating_sub(1) as u64);
    let ny = ny.min(dh.saturating_sub(1) as u64);
    (nx as i32, ny as i32)
}

/// Analiza un mensaje del protocolo y devuelve el clic que lleva.
///
/// Devuelve `None` si el mensaje no es valido o no es un clic.
pub fn analizar_clic(mensaje: &[u8]) -> Option<Clic> {
    if mensaje.len() < CABECERA {
        return None;
    }
    let magic = u32::from_le_bytes([mensaje[0], mensaje[1], mensaje[2], mensaje[3]]);
    if magic != MAGIC {
        return None;
    }
    if mensaje[4] != TIPO_CLIC {
        return None;
    }
    let tam = u16::from_le_bytes([mensaje[5], mensaje[6]]) as usize;
    if mensaje.len() < CABECERA + tam {
        return None;
    }
    let carga = &mensaje[CABECERA..CABECERA + tam];
    if carga.len() < 6 {
        return None;
    }
    Some(Clic {
        x: u16::from_le_bytes([carga[0], carga[1]]),
        y: u16::from_le_bytes([carga[2], carga[3]]),
        boton: carga[4],
        accion: carga[5],
    })
}

/// Analiza un mensaje de teclado.
pub fn analizar_tecla(mensaje: &[u8]) -> Option<Tecla> {
    if mensaje.len() < CABECERA {
        return None;
    }
    let magic = u32::from_le_bytes([mensaje[0], mensaje[1], mensaje[2], mensaje[3]]);
    if magic != MAGIC || mensaje[4] != TIPO_TECLA {
        return None;
    }
    let tam = u16::from_le_bytes([mensaje[5], mensaje[6]]) as usize;
    if mensaje.len() < CABECERA + tam || tam < 3 {
        return None;
    }
    let carga = &mensaje[CABECERA..CABECERA + tam];
    Some(Tecla {
        scancode: u16::from_le_bytes([carga[0], carga[1]]),
        accion: carga[2],
    })
}

/// Analiza un mensaje de rueda del raton.
pub fn analizar_rueda(mensaje: &[u8]) -> Option<Rueda> {
    if mensaje.len() < CABECERA {
        return None;
    }
    let magic = u32::from_le_bytes([mensaje[0], mensaje[1], mensaje[2], mensaje[3]]);
    if magic != MAGIC || mensaje[4] != TIPO_RUEDA {
        return None;
    }
    let tam = u16::from_le_bytes([mensaje[5], mensaje[6]]) as usize;
    if mensaje.len() < CABECERA + tam || tam < 4 {
        return None;
    }
    let carga = &mensaje[CABECERA..CABECERA + tam];
    Some(Rueda {
        x: i16::from_le_bytes([carga[0], carga[1]]),
        y: i16::from_le_bytes([carga[2], carga[3]]),
    })
}

/// Analiza un mensaje de movimiento del raton.
pub fn analizar_movimiento(mensaje: &[u8]) -> Option<Movimiento> {
    if mensaje.len() < CABECERA {
        return None;
    }
    let magic = u32::from_le_bytes([mensaje[0], mensaje[1], mensaje[2], mensaje[3]]);
    if magic != MAGIC || mensaje[4] != TIPO_MOVIMIENTO {
        return None;
    }
    let tam = u16::from_le_bytes([mensaje[5], mensaje[6]]) as usize;
    if mensaje.len() < CABECERA + tam || tam < 4 {
        return None;
    }
    let carga = &mensaje[CABECERA..CABECERA + tam];
    Some(Movimiento {
        x: u16::from_le_bytes([carga[0], carga[1]]),
        y: u16::from_le_bytes([carga[2], carga[3]]),
    })
}

pub fn scancode_a_sym(scancode: u16) -> i32 {
    match scancode {
        4 => b'a' as i32,
        7 => b'd' as i32,
        8 => b'e' as i32,
        20 => b'q' as i32,
        21 => b'r' as i32,
        22 => b's' as i32,
        26 => b'w' as i32,
        41 => 27, // SDLK_ESCAPE
        _ => scancode as i32,
    }
}

/// Construye el evento SDL2 de boton de raton.
///
/// El reparto de bytes es el de `SDL_MouseButtonEvent` en SDL2 (64 bits):
/// 0..4: type, 4..8: timestamp, 8..12: windowID, 12..16: which,
/// 16: button, 17: state, 18: clicks, 19: padding, 20..24: x, 24..28: y.
pub fn evento_boton(
    tipo: u32,
    window_id: u32,
    boton: u8,
    estado: u8,
    x: i32,
    y: i32,
) -> [u8; TAM_EVENTO] {
    let mut evento = [0u8; TAM_EVENTO];
    evento[0..4].copy_from_slice(&tipo.to_le_bytes());
    // 4..8: timestamp (0: el motor usa SDL_GetTicks si es necesario)
    evento[8..12].copy_from_slice(&window_id.to_le_bytes());
    // 12..16: which (0 = mouseID por defecto)
    evento[16] = boton;
    evento[17] = estado;
    evento[18] = 1; // numero de clics
    evento[20..24].copy_from_slice(&x.to_le_bytes());
    evento[24..28].copy_from_slice(&y.to_le_bytes());
    evento
}

/// Construye el evento SDL2 de movimiento de raton.
///
/// 0..4: type (0x0400 = SDL_MOUSEMOTION), 4..8: timestamp, 8..12: windowID,
/// 12..16: which, 16..20: state (botones pulsados), 20..24: x, 24..28: y,
/// 28..32: xrel, 32..36: yrel.
pub fn evento_movimiento(
    window_id: u32,
    estado_botones: u32,
    x: i32,
    y: i32,
) -> [u8; TAM_EVENTO] {
    let mut evento = [0u8; TAM_EVENTO];
    let tipo = 0x0400u32; // SDL_MOUSEMOTION
    evento[0..4].copy_from_slice(&tipo.to_le_bytes());
    evento[8..12].copy_from_slice(&window_id.to_le_bytes());
    evento[16..20].copy_from_slice(&estado_botones.to_le_bytes());
    evento[20..24].copy_from_slice(&x.to_le_bytes());
    evento[24..28].copy_from_slice(&y.to_le_bytes());
    evento
}

pub fn evento_tecla(
    tipo: u32,
    window_id: u32,
    scancode: u32,
    sym: i32,
    estado: u8,
) -> [u8; TAM_EVENTO] {
    let mut evento = [0u8; TAM_EVENTO];
    evento[0..4].copy_from_slice(&tipo.to_le_bytes());
    evento[8..12].copy_from_slice(&window_id.to_le_bytes());
    evento[12] = estado;
    evento[16..20].copy_from_slice(&scancode.to_le_bytes());
    evento[20..24].copy_from_slice(&sym.to_le_bytes());
    evento
}

pub fn evento_rueda(window_id: u32, x: i32, y: i32) -> [u8; TAM_EVENTO] {
    let mut evento = [0u8; TAM_EVENTO];
    let tipo = SDL_MOUSEWHEEL;
    evento[0..4].copy_from_slice(&tipo.to_le_bytes());
    evento[8..12].copy_from_slice(&window_id.to_le_bytes());
    evento[16..20].copy_from_slice(&x.to_le_bytes());
    evento[20..24].copy_from_slice(&y.to_le_bytes());
    evento
}

/// Instala el puente de entrada (resuelve SDL y abre el puerto).
///
/// Si `XFACTORVR_ENTRADA_PUERTO` no esta definido, escucha en 27021 por defecto.
pub fn instalar(simbolos: &Simbolos) {
    let puerto = std::env::var("XFACTORVR_ENTRADA_PUERTO")
        .unwrap_or_else(|_| "27021".to_string());

    let send_mouse_motion = match resolver_sdl(simbolos, &["SDL_SendMouseMotion"]) {
        Some(d) => d,
        None => {
            log::aviso("entrada: no se encontro SDL_SendMouseMotion: no hay puente de entrada");
            return;
        }
    };
    let get_mouse_focus = match resolver_sdl(simbolos, &["SDL_GetMouseFocus_REAL", "SDL_GetMouseFocus"]) {
        Some(d) => d,
        None => {
            log::aviso("entrada: no se encontro SDL_GetMouseFocus");
            return;
        }
    };
    let get_window_id = match resolver_sdl(simbolos, &["SDL_GetWindowID_REAL", "SDL_GetWindowID"]) {
        Some(d) => d,
        None => {
            log::aviso("entrada: no se encontro SDL_GetWindowID");
            return;
        }
    };
    let get_window_from_id =
        resolver_sdl(simbolos, &["SDL_GetWindowFromID_REAL", "SDL_GetWindowFromID"]);
    let get_window_size = resolver_sdl(simbolos, &["SDL_GetWindowSize_REAL", "SDL_GetWindowSize"]);
    let push_event = match resolver_sdl(simbolos, &["SDL_PushEvent_REAL", "SDL_PushEvent"]) {
        Some(d) => d,
        None => {
            log::aviso("entrada: no se encontro SDL_PushEvent");
            return;
        }
    };

    let sdl = Sdl {
        send_mouse_motion: unsafe { std::mem::transmute::<u64, unsafe extern "C" fn(*mut std::ffi::c_void, u32, i32, i32, i32) -> i32>(send_mouse_motion) },
        get_mouse_focus: unsafe { std::mem::transmute::<u64, unsafe extern "C" fn() -> *mut std::ffi::c_void>(get_mouse_focus) },
        get_window_id: unsafe { std::mem::transmute::<u64, unsafe extern "C" fn(*mut std::ffi::c_void) -> u32>(get_window_id) },
        get_window_from_id: match get_window_from_id {
            Some(d) => unsafe { std::mem::transmute::<u64, unsafe extern "C" fn(u32) -> *mut std::ffi::c_void>(d) },
            None => ventana_nula,
        },
        get_window_size: match get_window_size {
            Some(d) => unsafe { std::mem::transmute::<u64, unsafe extern "C" fn(*mut std::ffi::c_void, *mut i32, *mut i32)>(d) },
            None => tamano_nulo,
        },
        push_event: unsafe { std::mem::transmute::<u64, unsafe extern "C" fn(*const std::ffi::c_void) -> i32>(push_event) },
    };
    let _ = SDL.set(sdl);

    let puerto_num: u16 = match puerto.trim().parse() {
        Ok(p) => p,
        Err(_) => {
            log::aviso(&format!("entrada: puerto invalido '{puerto}'"));
            return;
        }
    };

    ACTIVO.store(1, Ordering::Relaxed);
    let manejador = std::thread::Builder::new()
        .name("xfv-entrada".to_string())
        .spawn(move || escuchar(puerto_num));
    if manejador.is_err() {
        log::aviso("entrada: no se pudo crear el hilo del puente de entrada");
        ACTIVO.store(0, Ordering::Relaxed);
    }
}

/// Funcion de reserva cuando el motor no trae `SDL_GetWindowFromID`.
unsafe extern "C" fn ventana_nula(_: u32) -> *mut std::ffi::c_void {
    std::ptr::null_mut()
}

/// Funcion de reserva cuando el motor no trae `SDL_GetWindowSize`.
unsafe extern "C" fn tamano_nulo(_: *mut std::ffi::c_void, w: *mut i32, h: *mut i32) {
    if !w.is_null() {
        unsafe { *w = 0 };
    }
    if !h.is_null() {
        unsafe { *h = 0 };
    }
}

/// Resuelve un simbolo de SDL probando los nombres que usa cada build.
///
/// El motor lleva SDL dentro, y en algunas compilaciones (la de Termux) los
/// simbolos reales llevan el sufijo `_REAL`.
fn resolver_sdl(simbolos: &Simbolos, candidatos: &[&str]) -> Option<u64> {
    for nombre in candidatos {
        if let Some(r) = simbolos.resolver(nombre) {
            return Some(r.direccion);
        }
        if let Some(r) = simbolos.resolver_exacto(nombre) {
            return Some(r.direccion);
        }
    }
    None
}

/// Escucha conexiones y aplica los clics.
fn escuchar(puerto: u16) {
    let direccion = format!("127.0.0.1:{puerto}");
    let escucha = match TcpListener::bind(&direccion) {
        Ok(l) => l,
        Err(e) => {
            log::error(&format!("entrada: no se pudo escuchar en {direccion}: {e}"));
            ACTIVO.store(0, Ordering::Relaxed);
            return;
        }
    };
    log::info(&format!("entrada: escuchando en {direccion}"));

    for conexion in escucha.incoming() {
        match conexion {
            Ok(c) => {
                let _ = c.set_nodelay(true);
                atender(c);
            }
            Err(e) => {
                log::aviso(&format!("entrada: error al aceptar: {e}"));
                break;
            }
        }
    }
}

/// Atiende una conexion: va sacando mensajes del flujo.
fn atender(mut conexion: TcpStream) {
    let mut buffer = [0u8; 1024];
    let acumulado: Vec<u8> = Vec::new();
    let mut acumulado = acumulado;
    loop {
        let leidos = match conexion.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => break,
        };
        acumulado.extend_from_slice(&buffer[..leidos]);
        // Sacar mensajes completos.
        loop {
            if acumulado.len() < CABECERA {
                break;
            }
            let tam = u16::from_le_bytes([acumulado[5], acumulado[6]]) as usize;
            let total = CABECERA + tam;
            if acumulado.len() < total {
                break;
            }
            let mensaje: Vec<u8> = acumulado.drain(..total).collect();
            if let Some(clic) = analizar_clic(&mensaje) {
                inyectar_clic(&clic);
            } else if let Some(tecla) = analizar_tecla(&mensaje) {
                inyectar_tecla(&tecla);
            } else if let Some(rueda) = analizar_rueda(&mensaje) {
                inyectar_rueda(&rueda);
            } else if let Some(mov) = analizar_movimiento(&mensaje) {
                inyectar_movimiento(&mov);
            }
        }
        // Si se acumularon demasiados bytes sin un mensaje valido, se limpian.
        if acumulado.len() > 4096 {
            acumulado.clear();
        }
    }
}

fn obtener_ventana(sdl: &Sdl) -> (*mut std::ffi::c_void, u32) {
    unsafe {
        let mut ventana = (sdl.get_mouse_focus)();
        if ventana.is_null() {
            ventana = (sdl.get_window_from_id)(1);
        }
        let mut id = if ventana.is_null() {
            0
        } else {
            (sdl.get_window_id)(ventana)
        };
        if id == 0 {
            id = 1;
        }
        (ventana, id)
    }
}

fn obtener_destino(sdl: &Sdl, ventana: *mut std::ffi::c_void) -> (u32, u32) {
    unsafe {
        let mut ww = 0i32;
        let mut wh = 0i32;
        if !ventana.is_null() {
            (sdl.get_window_size)(ventana, &mut ww, &mut wh);
        }
        if ww > 0 && wh > 0 {
            (ww as u32, wh as u32)
        } else {
            (
                VIEWPORT_W.load(Ordering::Relaxed),
                VIEWPORT_H.load(Ordering::Relaxed),
            )
        }
    }
}

/// Mete el clic en el motor como si fuera el raton.
fn inyectar_clic(clic: &Clic) {
    let sdl = match SDL.get() {
        Some(s) => s,
        None => return,
    };

    let origen = match crate::stream::Config::desde_entorno() {
        Some(c) => (c.ancho, c.alto),
        None => (0, 0),
    };

    let (ventana, window_id) = obtener_ventana(sdl);
    let destino = obtener_destino(sdl, ventana);
    let (x, y) = escalar(clic.x, clic.y, origen, destino);

    unsafe {
        // 1. Mover el raton del motor a esa posicion (ventana valida, relative = 0).
        (sdl.send_mouse_motion)(ventana, 0, 0, x, y);
        let ev_mov = evento_movimiento(window_id, 0, x, y);
        (sdl.push_event)(ev_mov.as_ptr() as *const std::ffi::c_void);

        // 2. Mandar el boton con los offsets exactos de SDL_MouseButtonEvent.
        let evento = match clic.accion {
            ACCION_BAJO => Some(evento_boton(
                SDL_MOUSEBUTTONDOWN,
                window_id,
                clic.boton,
                SDL_PRESSED,
                x,
                y,
            )),
            ACCION_ARRIBA => Some(evento_boton(
                SDL_MOUSEBUTTONUP,
                window_id,
                clic.boton,
                SDL_RELEASED,
                x,
                y,
            )),
            ACCION_CLIC => {
                let baja = evento_boton(
                    SDL_MOUSEBUTTONDOWN,
                    window_id,
                    clic.boton,
                    SDL_PRESSED,
                    x,
                    y,
                );
                (sdl.push_event)(baja.as_ptr() as *const std::ffi::c_void);
                Some(evento_boton(
                    SDL_MOUSEBUTTONUP,
                    window_id,
                    clic.boton,
                    SDL_RELEASED,
                    x,
                    y,
                ))
            }
            _ => None,
        };
        if let Some(ev) = evento {
            (sdl.push_event)(ev.as_ptr() as *const std::ffi::c_void);
        }
    }

    let n = CLICS.fetch_add(1, Ordering::Relaxed) + 1;
    log::info(&format!(
        "entrada: clic #{n} en ({x},{y}) boton={} accion={}",
        clic.boton, clic.accion
    ));
}

fn inyectar_tecla(tecla: &Tecla) {
    let sdl = match SDL.get() {
        Some(s) => s,
        None => return,
    };
    let (_, window_id) = obtener_ventana(sdl);
    let (tipo, estado) = if tecla.accion == ACCION_BAJO {
        (SDL_KEYDOWN, SDL_PRESSED)
    } else {
        (SDL_KEYUP, SDL_RELEASED)
    };
    let sym = scancode_a_sym(tecla.scancode);
    let ev = evento_tecla(tipo, window_id, tecla.scancode as u32, sym, estado);
    unsafe {
        (sdl.push_event)(ev.as_ptr() as *const std::ffi::c_void);
    }
    log::info(&format!(
        "entrada: tecla scancode={} accion={} (sym={})",
        tecla.scancode, tecla.accion, sym
    ));
}

fn inyectar_rueda(rueda: &Rueda) {
    let sdl = match SDL.get() {
        Some(s) => s,
        None => return,
    };
    let (_, window_id) = obtener_ventana(sdl);
    let ev = evento_rueda(window_id, rueda.x as i32, rueda.y as i32);
    unsafe {
        (sdl.push_event)(ev.as_ptr() as *const std::ffi::c_void);
    }
    log::info(&format!("entrada: rueda dx={} dy={}", rueda.x, rueda.y));
}

fn inyectar_movimiento(mov: &Movimiento) {
    let sdl = match SDL.get() {
        Some(s) => s,
        None => return,
    };
    let origen = match crate::stream::Config::desde_entorno() {
        Some(c) => (c.ancho, c.alto),
        None => (0, 0),
    };
    let (ventana, window_id) = obtener_ventana(sdl);
    let destino = obtener_destino(sdl, ventana);
    let (x, y) = escalar(mov.x, mov.y, origen, destino);
    unsafe {
        (sdl.send_mouse_motion)(ventana, 0, 0, x, y);
        let ev_mov = evento_movimiento(window_id, 0, x, y);
        (sdl.push_event)(ev_mov.as_ptr() as *const std::ffi::c_void);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mensaje_clic(x: u16, y: u16, boton: u8, accion: u8) -> Vec<u8> {
        let mut m = Vec::new();
        m.extend_from_slice(&MAGIC.to_le_bytes());
        m.push(TIPO_CLIC);
        m.extend_from_slice(&6u16.to_le_bytes());
        m.extend_from_slice(&x.to_le_bytes());
        m.extend_from_slice(&y.to_le_bytes());
        m.push(boton);
        m.push(accion);
        m
    }

    #[test]
    fn analiza_un_clic() {
        let m = mensaje_clic(120, 340, BOTON_IZQUIERDO, ACCION_CLIC);
        let c = analizar_clic(&m).expect("deberia analizar");
        assert_eq!(c.x, 120);
        assert_eq!(c.y, 340);
        assert_eq!(c.boton, BOTON_IZQUIERDO);
        assert_eq!(c.accion, ACCION_CLIC);
    }

    #[test]
    fn rechaza_lo_que_no_es_un_clic() {
        assert!(analizar_clic(&[]).is_none());
        assert!(analizar_clic(&[0, 1, 2, 3]).is_none());
        let mut m = mensaje_clic(1, 2, 1, ACCION_CLIC);
        m[0] = 0; // magic roto
        assert!(analizar_clic(&m).is_none());
        let m = mensaje_clic(1, 2, 1, ACCION_CLIC);
        assert!(analizar_clic(&m[..m.len() - 1]).is_none()); // carga incompleta
    }

    #[test]
    fn escala_de_la_imagen_al_framebuffer() {
        // Imagen de 640x360, framebuffer de 1280x720: el pixel se duplica.
        assert_eq!(escalar(100, 50, (640, 360), (1280, 720)), (200, 100));
        // El ultimo pixel se queda dentro.
        assert_eq!(escalar(639, 359, (640, 360), (1280, 720)), (1278, 718));
        // Sin tamano de origen no se escala.
        assert_eq!(escalar(100, 50, (0, 0), (1280, 720)), (100, 50));
    }

    #[test]
    fn el_evento_de_boton_tiene_el_formato_de_sdl2() {
        let e = evento_boton(SDL_MOUSEBUTTONDOWN, 7, BOTON_IZQUIERDO, SDL_PRESSED, 320, 180);
        assert_eq!(&e[0..4], &SDL_MOUSEBUTTONDOWN.to_le_bytes());
        assert_eq!(&e[8..12], &7u32.to_le_bytes());
        assert_eq!(e[16], BOTON_IZQUIERDO);
        assert_eq!(e[17], SDL_PRESSED);
        assert_eq!(e[18], 1);
        assert_eq!(&e[20..24], &320i32.to_le_bytes());
        assert_eq!(&e[24..28], &180i32.to_le_bytes());
    }

    #[test]
    fn analiza_tecla_y_rueda() {
        let mut m_t = Vec::new();
        m_t.extend_from_slice(&MAGIC.to_le_bytes());
        m_t.push(TIPO_TECLA);
        m_t.extend_from_slice(&3u16.to_le_bytes());
        m_t.extend_from_slice(&26u16.to_le_bytes()); // W
        m_t.push(ACCION_BAJO);
        let t = analizar_tecla(&m_t).expect("deberia analizar tecla");
        assert_eq!(t.scancode, 26);
        assert_eq!(t.accion, ACCION_BAJO);

        let mut m_r = Vec::new();
        m_r.extend_from_slice(&MAGIC.to_le_bytes());
        m_r.push(TIPO_RUEDA);
        m_r.extend_from_slice(&4u16.to_le_bytes());
        m_r.extend_from_slice(&0i16.to_le_bytes());
        m_r.extend_from_slice(&1i16.to_le_bytes()); // Rueda arriba
        let r = analizar_rueda(&m_r).expect("deberia analizar rueda");
        assert_eq!(r.x, 0);
        assert_eq!(r.y, 1);
    }
}

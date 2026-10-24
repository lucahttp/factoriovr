//! lua_bridge.rs — Hablar con el motor **por dentro**, sin RCON.
//!
//! # Por que
//!
//! El puente de RCON (puerto 27015) era el canal entre el visor y el motor:
//! el visor preguntaba, el motor respondia y el render esperaba (ADR-017
//! midio 61 ms por consulta).  Ademas, una orden por RCON la ejecuta el mod en
//! Lua, no el juego: no hay fantasma de construccion, ni rotacion, ni deshacer.
//!
//! El motor, en cambio, **exporta su API de Lua** (el binario no esta
//! stripeado: 291 301 simbolos).  Con ella, el inyector puede llamar a
//! `remote.call('xfactorvr', ...)` **en el mismo proceso del motor**: sin
//! sockets, sin sondeo, sin RCON y con la latencia de una llamada a funcion.
//!
//! # Como se consigue el `lua_State`
//!
//! `lua_pcallk` recibe el `lua_State*` en cada llamada, asi que engancharlo
//! (igual que se enganchan las entidades) nos da el estado.  Las llamadas
//! propias se ejecutan **fuera** de la pila Lua (en el hook de
//! `AppManager::process`, que corre en el hilo principal del juego) para no
//! reentrar.
//!
//! # API de Lua que hace falta (y el motor la tiene)
//!
//! | Simbolo | Para que |
//! |---|---|
//! | `lua_pcallk` | llamar y capturar el estado |
//! | `lua_rawgeti` | sacar la tabla de globales (`LUA_RIDX_GLOBALS`) |
//! | `lua_gettable` + `lua_pushstring` | llegar a `remote.call` por nombre |
//! | `lua_pushstring` / `lua_pushinteger` / `lua_pushnumber` | argumentos |
//! | `lua_tolstring` | leer el resultado (el JSON) |
//! | `lua_settop` | restaurar la pila (admite indices negativos) |

use std::ffi::c_void;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::detour;
use crate::log;
use crate::resolver::Simbolos;

/// `LUA_REGISTRYINDEX` (-10000).
const LUA_REGISTRYINDEX: i32 = -10000;
/// `LUA_RIDX_GLOBALS` (2).
const LUA_RIDX_GLOBALS: i32 = 2;

/// Numero de estados de Lua que se guardan (cada mod tiene el suyo).
const MAX_ESTADOS: usize = 4;

/// Direccion de la funcion de Lua que se llama (pcall).
type FnPcall = unsafe extern "C" fn(*mut c_void, i32, i32, i32) -> i32;
type FnGetTable = unsafe extern "C" fn(*mut c_void, i32);
type FnPushString = unsafe extern "C" fn(*mut c_void, *const u8) -> i32;
type FnToLString = unsafe extern "C" fn(*mut c_void, i32, *mut usize) -> *const u8;
type FnRawGetI = unsafe extern "C" fn(*mut c_void, i32, i32);
type FnPushInteger = unsafe extern "C" fn(*mut c_void, i64);
type FnPushNumber = unsafe extern "C" fn(*mut c_void, f64);
type FnSetTop = unsafe extern "C" fn(*mut c_void, i32);

static LUA_PCALLK: AtomicUsize = AtomicUsize::new(0);
static LUA_GETTABLE: AtomicUsize = AtomicUsize::new(0);
static LUA_PUSHSTRING: AtomicUsize = AtomicUsize::new(0);
static LUA_TOLSTRING: AtomicUsize = AtomicUsize::new(0);
static LUA_RAWGETI: AtomicUsize = AtomicUsize::new(0);
static LUA_PUSHINTEGER: AtomicUsize = AtomicUsize::new(0);
static LUA_PUSHNUMBER: AtomicUsize = AtomicUsize::new(0);
static LUA_SETTOP: AtomicUsize = AtomicUsize::new(0);

/// Trampolin de `lua_pcallk`.
static TRAMPOLIN_PCALLK: AtomicUsize = AtomicUsize::new(0);
/// Trampolin de `lua_pushstring`.
static TRAMPOLIN_PUSHSTRING: AtomicUsize = AtomicUsize::new(0);
/// Trampolin de `lua_rawgeti`.
static TRAMPOLIN_RAWGETI: AtomicUsize = AtomicUsize::new(0);
/// Trampolin de `lua_gettable`.
static TRAMPOLIN_GETTABLE: AtomicUsize = AtomicUsize::new(0);

/// Estados de Lua vistos (el ultimo es el mas reciente).
static ESTADOS: [AtomicUsize; MAX_ESTADOS] = [
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
    AtomicUsize::new(0),
];
/// Cuantos estados hay capturados.
static N_ESTADOS: AtomicUsize = AtomicUsize::new(0);

/// Un argumento para una llamada a Lua.
#[derive(Clone, Debug)]
pub enum Arg {
    /// Cadena de texto.
    Texto(String),
    /// Numero entero.
    Entero(i64),
    /// Numero con decimales.
    Numero(f64),
}

impl Arg {
    fn empujar(&self, l: *mut c_void, p: &Fns) {
        unsafe {
            match self {
                Arg::Texto(s) => {
                    // lua_pushstring copia la cadena, pero necesita un NUL.
                    let mut bytes = s.as_bytes().to_vec();
                    bytes.push(0);
                    (p.push_string)(l, bytes.as_ptr());
                }
                Arg::Entero(i) => (p.push_integer)(l, *i),
                Arg::Numero(n) => (p.push_number)(l, *n),
            }
        }
    }
}

/// Tabla de punteros a las funciones de Lua resueltas.
struct Fns {
    pcall: FnPcall,
    get_table: FnGetTable,
    push_string: FnPushString,
    to_lstring: FnToLString,
    raw_get_i: FnRawGetI,
    push_integer: FnPushInteger,
    push_number: FnPushNumber,
    set_top: FnSetTop,
}

/// Hook de `lua_pcallk`: captura el `lua_State*` y deja pasar la llamada.
#[no_mangle]
pub extern "C" fn xfv_hook_lua_pcallk(
    estado: *mut c_void,
    nargs: i32,
    nresults: i32,
    errfunc: i32,
) -> i32 {
    if !estado.is_null() {
        recordar_estado(estado);
    }
    unsafe {
        let t = TRAMPOLIN_PCALLK.load(Ordering::SeqCst);
        if t == 0 {
            return 0;
        }
        let f: FnPcall = std::mem::transmute(t);
        f(estado, nargs, nresults, errfunc)
    }
}

/// Contadores de llamadas de cada hook (para el diagnostico).
static LLAMADAS_PUSHSTRING: AtomicUsize = AtomicUsize::new(0);
static LLAMADAS_RAWGETI: AtomicUsize = AtomicUsize::new(0);
static LLAMADAS_GETTABLE: AtomicUsize = AtomicUsize::new(0);
static LLAMADAS_PCALLK: AtomicUsize = AtomicUsize::new(0);

/// Hook de `lua_pushstring`: el motor lo llama en cada operacion con el motor.
///
/// `lua_pcallk` NO sirve para capturar el estado: en Lua 5.4 `lua_call` va a
/// `luaD_call` (interna) y no pasa por `lua_pcallk`.  Las funciones que el
/// motor SI usa desde C++ (el binder de `LuaGameScript`) son estas, y todas
/// reciben el `lua_State*` como primer argumento.
#[no_mangle]
pub extern "C" fn xfv_hook_lua_pushstring(estado: *mut c_void, texto: *const u8) -> i32 {
    LLAMADAS_PUSHSTRING.fetch_add(1, Ordering::Relaxed);
    if !estado.is_null() {
        recordar_estado(estado);
    }
    unsafe {
        let t = TRAMPOLIN_PUSHSTRING.load(Ordering::SeqCst);
        if t == 0 {
            return 0;
        }
        let f: FnPushString = std::mem::transmute(t);
        f(estado, texto)
    }
}

/// Hook de `lua_rawgeti`.
#[no_mangle]
pub extern "C" fn xfv_hook_lua_rawgeti(estado: *mut c_void, indice: i32, clave: i32) {
    if !estado.is_null() {
        recordar_estado(estado);
    }
    unsafe {
        let t = TRAMPOLIN_RAWGETI.load(Ordering::SeqCst);
        if t == 0 {
            return;
        }
        let f: FnRawGetI = std::mem::transmute(t);
        f(estado, indice, clave)
    }
}

/// Hook de `lua_gettable`.
#[no_mangle]
pub extern "C" fn xfv_hook_lua_gettable(estado: *mut c_void, indice: i32) {
    if !estado.is_null() {
        recordar_estado(estado);
    }
    unsafe {
        let t = TRAMPOLIN_GETTABLE.load(Ordering::SeqCst);
        if t == 0 {
            return;
        }
        let f: FnGetTable = std::mem::transmute(t);
        f(estado, indice)
    }
}

/// ID del hilo principal del juego (el que corre el bucle y la Lua).
///
/// El motor tiene Lua en VARIOS hilos (carga de mods, red, script remoto).
/// Un `lua_State*` capturado en otro hilo no se puede usar desde aqui: el
/// estado pertenece a ese hilo y llamarlo desde otro revienta el motor
/// ("invalid index" en `lua_rawgeti`, SIGABRT).  Por eso solo se guardan los
/// estados del hilo principal, que es el que el motor usa para los mods.
static HILO_PRINCIPAL: AtomicUsize = AtomicUsize::new(0);

/// `gettid(2)` de Linux: el identificador del hilo actual.
///
/// Solo existe en Linux (el inyector corre en el dispositivo); en el host, para
/// los bancos, se usa 0 (= "sin filtro de hilo").
#[cfg(target_os = "linux")]
extern "C" {
    fn gettid() -> i32;
}

/// ID del hilo actual (0 si no se puede saber).
#[cfg(target_os = "linux")]
fn tid_actual() -> usize {
    (unsafe { gettid() }) as usize
}

#[cfg(not(target_os = "linux"))]
fn tid_actual() -> usize {
    0
}

/// Remember a Lua state (only from the main thread).
fn recordar_estado(estado: *mut c_void) {
    // Solo el hilo principal: los demas tienen su propio lua_State y no se
    // pueden tocar desde aqui.
    let principal = HILO_PRINCIPAL.load(Ordering::Relaxed);
    if principal != 0 && tid_actual() != principal {
        return;
    }
    let addr = estado as usize;
    for i in 0..MAX_ESTADOS {
        if ESTADOS[i].load(Ordering::Relaxed) == addr {
            return;
        }
    }
    for i in (1..MAX_ESTADOS).rev() {
        ESTADOS[i].store(ESTADOS[i - 1].load(Ordering::Relaxed), Ordering::Relaxed);
    }
    ESTADOS[0].store(addr, Ordering::Relaxed);
    let n = N_ESTADOS.load(Ordering::Relaxed);
    if n < MAX_ESTADOS {
        N_ESTADOS.store(n + 1, Ordering::Relaxed);
    }
}

/// Fija cual es el hilo principal del motor (se llama desde su bucle).
pub fn fijar_hilo_principal() {
    let t = tid_actual();
    if HILO_PRINCIPAL.load(Ordering::Relaxed) != t {
        HILO_PRINCIPAL.store(t, Ordering::Relaxed);
        log::info(&format!("lua_bridge: hilo principal del motor: {t}"));
    }
}

/// Hay algun estado de Lua capturado?
pub fn hay_estado() -> bool {
    N_ESTADOS.load(Ordering::Relaxed) > 0
}

/// Instala el bridge de Lua (resuelve los simbolos y engancha `lua_pcallk`).
pub fn instalar(simbolos: &Simbolos) -> bool {
    macro_rules! sym {
        ($nombre:expr, $destino:expr) => {
            match simbolos.resolver_exacto($nombre) {
                Some(r) => {
                    $destino.store(r.direccion as usize, Ordering::SeqCst);
                }
                None => {
                    log::aviso(&format!("lua_bridge: no se encontro {}", $nombre));
                    return false;
                }
            }
        };
    }
    sym!("lua_pcallk", LUA_PCALLK);
    sym!("lua_gettable", LUA_GETTABLE);
    sym!("lua_pushstring", LUA_PUSHSTRING);
    sym!("lua_tolstring", LUA_TOLSTRING);
    sym!("lua_rawgeti", LUA_RAWGETI);
    sym!("lua_pushinteger", LUA_PUSHINTEGER);
    sym!("lua_pushnumber", LUA_PUSHNUMBER);
    sym!("lua_settop", LUA_SETTOP);

    let pcall = simbolos.resolver_exacto("lua_pcallk").expect("resuelto antes");
    let hook = xfv_hook_lua_pcallk as *const () as u64;
    match detour::instalar(pcall.direccion, hook, None, pcall.tamano) {
        Ok(d) => {
            TRAMPOLIN_PCALLK.store(d.trampolin as usize, Ordering::SeqCst);
            log::info("lua_bridge: enganchado lua_pcallk (trampolin en el estado de Lua)");
        }
        Err(e) => {
            log::error(&format!("lua_bridge: no se pudo enganchar lua_pcallk: {e}"));
        }
    }

    // Ademas de `lua_pcallk` (que el motor no usa: en Lua 5.4 `lua_call` va a
    // `luaD_call`, interna), se enganchan las funciones que el binder de
    // `LuaGameScript` SI llama desde C++: todas reciben el estado como primer
    // argumento, asi que el hook es el mismo.
    let mut enganchados = 0;
    if let Some(r) = simbolos.resolver_exacto("lua_pushstring") {
        let h = xfv_hook_lua_pushstring as *const () as u64;
        if let Ok(d) = detour::instalar(r.direccion, h, None, r.tamano) {
            TRAMPOLIN_PUSHSTRING.store(d.trampolin as usize, Ordering::SeqCst);
            enganchados += 1;
        }
    }
    if let Some(r) = simbolos.resolver_exacto("lua_rawgeti") {
        let h = xfv_hook_lua_rawgeti as *const () as u64;
        if let Ok(d) = detour::instalar(r.direccion, h, None, r.tamano) {
            TRAMPOLIN_RAWGETI.store(d.trampolin as usize, Ordering::SeqCst);
            enganchados += 1;
        }
    }
    if let Some(r) = simbolos.resolver_exacto("lua_gettable") {
        let h = xfv_hook_lua_gettable as *const () as u64;
        if let Ok(d) = detour::instalar(r.direccion, h, None, r.tamano) {
            TRAMPOLIN_GETTABLE.store(d.trampolin as usize, Ordering::SeqCst);
            enganchados += 1;
        }
    }
    log::info(&format!("lua_bridge: {enganchados} hooks de captura del estado de Lua"));
    true
}

/// Llama a una funcion de la interfaz `xfactorvr` del mod, en proceso.
///
/// Devuelve el texto que devuelve la funcion (el JSON del snapshot, por
/// ejemplo) o `None` si la llamada fallo.
pub fn llamar_xfactorvr(funcion: &str, args: &[Arg]) -> Option<String> {
    let total = N_ESTADOS.load(Ordering::Relaxed).min(MAX_ESTADOS);
    let mut ultimo_error = String::new();
    for i in 0..total {
        let addr = ESTADOS[i].load(Ordering::Relaxed);
        if addr == 0 {
            continue;
        }
        match llamar_en(addr as *mut c_void, funcion, args) {
            Some(r) => return Some(r),
            None => {
                ultimo_error = format!("el estado {i} (0x{addr:x}) no respondio");
            }
        }
    }
    log::aviso(&format!(
        "lua_bridge: no se pudo llamar a {funcion} con ningun estado ({total} estados): {ultimo_error}"
    ));
    None
}

/// Hace la llamada en un estado concreto.
fn llamar_en(estado: *mut c_void, funcion: &str, args: &[Arg]) -> Option<String> {
    let p = Fns {
        pcall: unsafe { std::mem::transmute(LUA_PCALLK.load(Ordering::SeqCst)) },
        get_table: unsafe { std::mem::transmute(LUA_GETTABLE.load(Ordering::SeqCst)) },
        push_string: unsafe { std::mem::transmute(LUA_PUSHSTRING.load(Ordering::SeqCst)) },
        to_lstring: unsafe { std::mem::transmute(LUA_TOLSTRING.load(Ordering::SeqCst)) },
        raw_get_i: unsafe { std::mem::transmute(LUA_RAWGETI.load(Ordering::SeqCst)) },
        push_integer: unsafe { std::mem::transmute(LUA_PUSHINTEGER.load(Ordering::SeqCst)) },
        push_number: unsafe { std::mem::transmute(LUA_PUSHNUMBER.load(Ordering::SeqCst)) },
        set_top: unsafe { std::mem::transmute(LUA_SETTOP.load(Ordering::SeqCst)) },
    };
    if (p.pcall as usize) == 0 {
        return None;
    }

    unsafe {
        // 1. Tabla de globales.
        (p.raw_get_i)(estado, LUA_REGISTRYINDEX, LUA_RIDX_GLOBALS);
        // 2. remote = globals["remote"]
        let mut k = b"remote\0".to_vec();
        (p.push_string)(estado, k.as_ptr());
        (p.get_table)(estado, -2);
        // 3. call = remote["call"]
        k = b"call\0".to_vec();
        (p.push_string)(estado, k.as_ptr());
        (p.get_table)(estado, -2);
        // Si `remote` o `remote.call` no existen en este estado, `lua_gettable`
        // deja nil y la llamada falla: se avisa con el codigo de error.
        // 4. Argumentos: interfaz, funcion y los del llamante.
        let mut todos: Vec<Arg> = Vec::with_capacity(args.len() + 2);
        todos.push(Arg::Texto("xfactorvr".to_string()));
        todos.push(Arg::Texto(funcion.to_string()));
        todos.extend_from_slice(args);
        for a in &todos {
            a.empujar(estado, &p);
        }
        // 5. La llamada: la funcion esta en -(nargs+1).
        let nargs = todos.len() as i32;
        let rc = (p.pcall)(estado, nargs, 1, 0);
        // 6. Resultado (o mensaje de error) y restauracion de la pila.
        let mut largo: usize = 0;
        let ptr = (p.to_lstring)(estado, -1, &mut largo);
        let texto = if ptr.is_null() {
            String::new()
        } else {
            let bytes = std::slice::from_raw_parts(ptr, largo);
            String::from_utf8_lossy(bytes).to_string()
        };
        // La pila queda con [globales, remote, resultado]: se desapilan 3.
        (p.set_top)(estado, -4);
        if rc != 0 {
            log::aviso(&format!(
                "lua_bridge: la llamada a {funcion} fallo (rc={rc}): {texto}"
            ));
            return None;
        }
        Some(texto)
    }
}

/// Pide el estado del mundo al mod (el mismo JSON que devolvia por RCON).
pub fn pedir_mundo(radio: f64, cx: f64, cy: f64) -> Option<String> {
    llamar_xfactorvr(
        "get_world_snapshot",
        &[
            Arg::Numero(radio),
            Arg::Numero(cx),
            Arg::Numero(cy),
        ],
    )
}

/// Pide el inventario del jugador al mod.
pub fn pedir_inventario() -> Option<String> {
    llamar_xfactorvr("get_inventory", &[])
}

/// Ejecuta una orden del mod (minar, colocar, interactuar…).
pub fn ejecutar_orden(funcion: &str, args: &[Arg]) -> Option<String> {
    llamar_xfactorvr(funcion, args)
}

/// Contador de frames (para el muestreo).
static FRAMES: AtomicUsize = AtomicUsize::new(0);
/// Ya se ha avisado de que el puente funciona (una vez).
static AVISADO: AtomicUsize = AtomicUsize::new(0);
/// Ultimo estado del mundo gotten en proceso.
static ULTIMO_MUNDO: std::sync::Mutex<Option<String>> = std::sync::Mutex::new(None);

/// Se llama una vez por frame desde el hook de eventos.
///
/// Hace UNA llamada al mod por segundo (muestra de diagnostico).  Cuando el
/// puente este probado, aqui es donde se pediria la escena para mandarla al
/// visor por el socket (en vez de por RCON).
pub fn tick() {
    if std::env::var("XFACTORVR_LUA_BRIDGE").ok().as_deref() != Some("1") {
        return;
    }
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
    if n % 60 != 0 {
        return;
    }
    if AVISADO.swap(1, Ordering::SeqCst) == 0 {
        log::info(&format!(
            "lua_bridge: estados={} pushstring={} rawgeti={} gettable={} pcallk={}",
            N_ESTADOS.load(Ordering::Relaxed),
            LLAMADAS_PUSHSTRING.load(Ordering::Relaxed),
            LLAMADAS_RAWGETI.load(Ordering::Relaxed),
            LLAMADAS_GETTABLE.load(Ordering::Relaxed),
            LLAMADAS_PCALLK.load(Ordering::Relaxed),
        ));
    }
    if !hay_estado() {
        return;
    }
    let mundo = pedir_mundo(32.0, 0.0, 0.0);
    if let Some(m) = mundo {
        let largo = m.len();
        if let Ok(mut g) = ULTIMO_MUNDO.lock() {
            *g = Some(m);
        }
        log::info(&format!(
            "lua_bridge: llamada en proceso al mod OK ({largo} bytes de estado)"
        ));
    }
}

/// Ultimo estado del mundo gotten en proceso (si lo hay).
pub fn ultimo_mundo() -> Option<String> {
    ULTIMO_MUNDO.lock().ok().and_then(|g| g.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn los_argumentos_se_convierten_a_las_cajas_correctas() {
        // El texto va con NUL al final (lua_pushstring lo necesita).
        let a = Arg::Texto("hola".to_string());
        match &a {
            Arg::Texto(s) => {
                let mut bytes = s.as_bytes().to_vec();
                bytes.push(0);
                assert_eq!(&bytes[..4], b"hola");
                assert_eq!(bytes[4], 0);
            }
            _ => panic!("deberia ser texto"),
        }
    }

    #[test]
    fn sin_estado_de_lua_no_hay_respuesta() {
        // No se ha enganchado nada: la llamada debe fallar sin reventar.
        assert!(llamar_xfactorvr("get_world_snapshot", &[]).is_none());
    }

    #[test]
    fn los_estados_se_recuerdan_sin_duplicados() {
        ESTADOS[0].store(0, Ordering::Relaxed);
        N_ESTADOS.store(0, Ordering::Relaxed);
        recordar_estado(0x1000 as *mut c_void);
        recordar_estado(0x2000 as *mut c_void);
        recordar_estado(0x1000 as *mut c_void);
        assert_eq!(N_ESTADOS.load(Ordering::Relaxed), 2);
        assert_eq!(ESTADOS[0].load(Ordering::Relaxed), 0x2000);
    }
}

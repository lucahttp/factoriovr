//! Diagnostico de la pila de estados del `AppManager`.
//!
//! Sirve para ver **en que estado esta el motor cuando la carga no avanza**,
//! sin tocar nada: solo lee memoria y escribe un par de lineas por stderr.
//!
//! Se activa con `XFACTORVR_DIAG_PILA=1` y vuelca la pila cada 120 ticks de
//! `AppManager::process()` (que se llama una vez por frame).
//!
//! Que se mira:
//!
//!   * `AppManager+0x58`: el "estado de la app" (0 = ninguno, 1 = hay una
//!     partida cargandose, ...).
//!   * La pila de estados (offset 0x258..0x260): el puntero de cada estado y
//!     su tabla virtual. Los estados se identifican **por la tabla virtual**
//!     (comparada con `_ZTV...`), no llamando a `getStateType()`: en esa
//!     pila hay punteros a estados ya destruidos, y llamar a un metodo
//!     virtual sobre uno de ellos revienta el juego (SIGSEGV).
//!   * Si hay un `InLoadingGameBox`, el puntero al cargador
//!     (`ParallelScenarioLoader`, offset 0x18), su estado (0x78) y el flag de
//!     "cargado" (0xa8). Con eso se ve si el hilo del cargador sigue vivo o si
//!     el motor ya se ha rindido.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::fase_juego;
use crate::log;
use crate::resolver::Simbolos;

/// Contador de ticks del diagnostico.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Tabla virtual de `InLoadingGameBox` (el puntero que lleva el objeto).
static VTABLE_CARGA: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InGame`.
static VTABLE_INGAME: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InMainMenu`.
static VTABLE_MENU: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InUpdater`.
static VTABLE_UPDATER: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InGenerateMapDialog`.
static VTABLE_GENERAR: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InCreatingGame`.
static VTABLE_CREANDO: AtomicUsize = AtomicUsize::new(0);
/// Tabla virtual de `InLoadGameDialog`.
static VTABLE_CARGAR_PARTIDA: AtomicUsize = AtomicUsize::new(0);

/// Direccion del puntero global al `GlobalContext`.
///
/// En el binario del motor es el simbolo `global` (unico en el `.symtab`) y
/// lo lee `MainLoop::run()` con `ldr x8, [x28, #0x6d0]`. De ahi se sacan los
/// campos que le importan a `InGame::process()`:
///   * `+0x78`  : el `Scenario` que se esta cargando,
///   * `+0x2b8` : la vista de control (la crea el `InLoadGameDialog`),
///   * `+0x2c0` : la vista de control activa.
static GLOBAL_CTX: AtomicUsize = AtomicUsize::new(0);

/// Offset (en el fichero) del puntero global al `GlobalContext`.
const OFFSET_GLOBAL_CTX: u64 = 0x3eb96d0;

/// Offset del puntero a la vtable dentro del objeto (16 bytes de cabecera de
/// la Itanium ABI: vtable pointer + offset to top).
const OFFSET_VTABLE_OBJETO: u64 = 16;

/// Cada cuantos ticks se vuelca la pila.
const CADA_TICKS: u64 = 120;

/// Milisegundos desde el arranque (para ordenar los volcados).
static INICIO_MS: AtomicU64 = AtomicU64::new(0);

/// Resuelve las tablas virtuales que se usan para identificar los estados.
///
/// Se llama una vez, al instalar los hooks (el binario del motor no esta
/// stripeado, asi que las tablas estan en el `.symtab`).
pub fn resolver_vtables(simbolos: &Simbolos) {
    for (mangled, destino) in [
        ("_ZTV16InLoadingGameBox", &VTABLE_CARGA),
        ("_ZTV6InGame", &VTABLE_INGAME),
        ("_ZTV10InMainMenu", &VTABLE_MENU),
        ("_ZTV9InUpdater", &VTABLE_UPDATER),
        ("_ZTV19InGenerateMapDialog", &VTABLE_GENERAR),
        ("_ZTV13InCreatingGame", &VTABLE_CREANDO),
        ("_ZTV16InLoadGameDialog", &VTABLE_CARGAR_PARTIDA),
    ] {
        if let Some(r) = simbolos.resolver_exacto(mangled) {
            destino.store(
                (r.direccion + OFFSET_VTABLE_OBJETO) as usize,
                Ordering::SeqCst,
            );
        }
    }
    // El puntero global al `GlobalContext`.
    //
    // OJO: en el `.symtab` hay DOS simbolos llamados `global` (uno es una
    // variable local de `get_global()`), asi que el resolutor por nombre coge
    // el equivocado. Se usa el offset deducido del desensamblado de
    // `MainLoop::run()`: `adrp x23, 0x3eb9000` + `ldr x9, [x23, #0x6d0]`.
    GLOBAL_CTX.store((simbolos.load_bias + OFFSET_GLOBAL_CTX) as usize, Ordering::SeqCst);
}

/// El `GlobalContext` (0 si no se ha podido).
fn global_context() -> u64 {
    let dir = GLOBAL_CTX.load(Ordering::SeqCst);
    if dir == 0 {
        return 0;
    }
    // SAFETY: el global contiene un puntero al `GlobalContext`.
    unsafe { (dir as *const u64).read() }
}

/// El valor crudo del global (para depurar la resolucion).
pub fn global_crudo() -> u64 {
    let dir = GLOBAL_CTX.load(Ordering::SeqCst);
    if dir == 0 {
        return 0;
    }
    // SAFETY: lectura pura del global.
    unsafe { (dir as *const u64).read() }
}

/// Un campo del `GlobalContext` por offset (0 si no hay `GlobalContext`).
pub fn campo_global(offset: u64) -> u64 {
    let gc = global_context();
    if gc == 0 {
        return 0;
    }
    // SAFETY: lectura pura del `GlobalContext`.
    unsafe { (gc as *const u8).add(offset as usize).cast::<u64>().read() }
}

/// Lee una `std::string` de la memoria (16 bytes: puntero + longitud).
///
/// Las cadenas cortas (SSO) guardan los datos dentro del propio objeto, asi
/// que hay que mirar la longitud para saber de donde leer.
pub fn leer_std_string(direccion: u64) -> String {
    if direccion == 0 {
        return String::new();
    }
    // SAFETY: lectura pura de la cadena.
    unsafe {
        let puntero = (direccion as *const u64).read();
        let longitud = (direccion as *const u64).add(1).read() as usize;
        if longitud > 4096 {
            return String::new();
        }
        let datos = if longitud <= 15 {
            direccion as *const u8
        } else {
            puntero as *const u8
        };
        let slice = std::slice::from_raw_parts(datos, longitud);
        String::from_utf8_lossy(slice).to_string()
    }
}

/// La configuracion del RCON que lee `AppManager::startRconIfEnabled()`.
///
/// La cadena del puerto esta en `GlobalContext+0x240` -> `+0x2348` y la
/// password en `+0x2428` (longitud) y `+0x2430` (puntero).
pub fn config_rcon() -> (String, String) {
    let ajustes = campo_global(0x240);
    if ajustes == 0 {
        return (String::new(), String::new());
    }
    let puerto = leer_std_string(ajustes + 0x2348);
    let long_pass = unsafe { (ajustes as *const u64).add(0x2428 / 8).read() } as usize;
    let pun_pass = unsafe { (ajustes as *const u64).add(0x2430 / 8).read() };
    let password = if long_pass <= 4096 {
        if long_pass <= 15 {
            leer_std_string(ajustes + 0x2428)
        } else if pun_pass != 0 {
            // SAFETY: la cadena apunta a `long_pass` bytes.
            unsafe {
                let slice = std::slice::from_raw_parts(pun_pass as *const u8, long_pass);
                String::from_utf8_lossy(slice).to_string()
            }
        } else {
            String::new()
        }
    } else {
        String::new()
    };
    (puerto, password)
}

/// Cada cuantos ticks se vuelca la pila (`XFACTORVR_DIAG_CADA`, por defecto 120).
fn cada_ticks() -> u64 {
    match std::env::var("XFACTORVR_DIAG_CADA") {
        Ok(v) => v.parse::<u64>().unwrap_or(CADA_TICKS).max(1),
        Err(_) => CADA_TICKS,
    }
}

/// Si el diagnostico esta activado.
pub fn habilitado() -> bool {
    matches!(std::env::var("XFACTORVR_DIAG_PILA"), Ok(v) if v == "1")
}

/// Vuelca la pila de estados (llamar desde el hook de `AppManager::process`).
pub fn tick(app_manager: *mut c_void) {
    if app_manager.is_null() {
        return;
    }
    let ticks = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    if ticks == 1 {
        INICIO_MS.store(ahora_ms(), Ordering::SeqCst);
    }
    let cada = cada_ticks();
    if ticks % cada != 0 && ticks != 1 {
        return;
    }
    volcar(app_manager, ticks);
}

/// Milisegundos desde la epoca.
fn ahora_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// Vuelca el estado interno de la carga.
fn volcar(app_manager: *mut c_void, ticks: u64) {
    let ms = ahora_ms().saturating_sub(INICIO_MS.load(Ordering::SeqCst));
    log::info(&format!(
        "diag: tick={ticks} t={ms}ms estados={} app_estado={}",
        numero_estados(app_manager),
        fase_juego::estado_app(app_manager),
    ));
    let (puerto, password) = config_rcon();
    log::info(&format!(
        "diag:   global: dir=0x{:x} ctx=0x{:x} escenario=0x{:x} vista_control=0x{:x} vista_activa=0x{:x}",
        global_crudo(),
        global_context(),
        campo_global(0x78),
        campo_global(0x2b8),
        campo_global(0x2c0),
    ));
    log::info(&format!(
        "diag:   rcon: puerto='{puerto}' password='{}'",
        if password.is_empty() {
            "(vacia)"
        } else {
            &password
        }
    ));

    // La pila entera, de abajo arriba.
    let inicio = fase_juego::inicio_pila(app_manager);
    let fin = fase_juego::fin_pila(app_manager);
    if inicio.is_null() || fin.is_null() || inicio >= fin {
        log::info("diag: pila de estados vacia o invalida");
        return;
    }
    let mut p = inicio;
    let mut i = 0usize;
    while p < fin {
        let estado = unsafe { *p };
        if estado == 0 {
            log::info(&format!("diag:   [{i}] estado=NULO"));
        } else {
            // Solo se LEE la vtable: nunca se llama a un metodo virtual de un
            // estado (puede estar destruido y el juego se SEGSEGV).
            let vtable = unsafe { *(estado as *const u64) };
            log::info(&format!(
                "diag:   [{i}] estado=0x{estado:016x} vtable=0x{vtable:016x} tipo={}",
                nombre_estado(vtable)
            ));
            if vtable == VTABLE_CARGA.load(Ordering::Relaxed) as u64 {
                volcar_caja(estado as *mut c_void);
            }
        }
        p = unsafe { p.add(1) };
        i += 1;
    }
}

/// Si el estado de la cima de la pila es un `InGame`.
///
/// Se usa para saber si la partida esta en marcha (el motor solo llama al
/// `process()` del estado de la cima).
pub fn es_in_game(app_manager: *mut c_void) -> bool {
    let cima = fase_juego::estado_cima(app_manager);
    if cima.is_null() {
        return false;
    }
    // SAFETY: lectura pura de memoria: la vtable del objeto de la cima.
    let vtable = unsafe { *(cima as *const u64) };
    vtable != 0 && vtable == VTABLE_INGAME.load(Ordering::Relaxed) as u64
}

/// Nombre del estado segun su tabla virtual.
fn nombre_estado(vtable: u64) -> &'static str {    let v = vtable as usize;
    if v == 0 {
        return "NULO";
    }
    if v == VTABLE_CARGA.load(Ordering::Relaxed) {
        "InLoadingGameBox"
    } else if v == VTABLE_INGAME.load(Ordering::Relaxed) {
        "InGame"
    } else if v == VTABLE_MENU.load(Ordering::Relaxed) {
        "InMainMenu"
    } else if v == VTABLE_UPDATER.load(Ordering::Relaxed) {
        "InUpdater"
    } else if v == VTABLE_GENERAR.load(Ordering::Relaxed) {
        "InGenerateMapDialog"
    } else if v == VTABLE_CREANDO.load(Ordering::Relaxed) {
        "InCreatingGame"
    } else if v == VTABLE_CARGAR_PARTIDA.load(Ordering::Relaxed) {
        "InLoadGameDialog"
    } else {
        "?"
    }
}

/// Vuelca el estado del cargador (`ParallelScenarioLoader`) de una caja de carga.
fn volcar_caja(estado: *mut c_void) {
    // SAFETY: la caja de carga tiene el cargador en el offset 0x18.
    let cargador = unsafe { (estado as *const u8).add(0x18).cast::<u64>().read() };
    if cargador == 0 {
        log::info("diag:     caja SIN cargador (cargador=0)");
        return;
    }
    // SAFETY: offsets deducidos del desensamblado del motor.
    let (estado_cargador, flag) = unsafe {
        (
            (cargador as *const u8).add(0x78).cast::<u32>().read(),
            (cargador as *const u8).add(0xa8).cast::<u32>().read(),
        )
    };
    log::info(&format!(
        "diag:     cargador=0x{cargador:016x} estado_cargador={estado_cargador} flag={flag}"
    ));
}

/// Numero de estados de la pila.
fn numero_estados(app_manager: *mut c_void) -> usize {
    let inicio = fase_juego::inicio_pila(app_manager);
    let fin = fase_juego::fin_pila(app_manager);
    if inicio.is_null() || fin.is_null() || inicio >= fin {
        0
    } else {
        (fin as usize - inicio as usize) / 8
    }
}

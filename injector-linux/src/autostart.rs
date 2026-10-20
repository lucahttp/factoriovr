//! Arranque automatico de una partida dentro del proceso del cliente grafico.
//!
//! # Por que hace falta
//!
//! El cliente grafico de Factorio 2.1.22 no tiene ninguna opcion de linea de
//! comandos para abrir una partida en single-player:
//!   * `--start-server` y `--start-server-load-scenario` conmutan el motor a
//!     "Running in headless mode" (sin graficos), y
//!   * sin ellos el cliente se queda en el MENU, sin abrir RCON.
//!
//! Ademas, un nivel de campana necesita un JUGADOR real: su `control.lua` usa
//! `game.players[1]`, que en un servidor headless no existe.
//!
//! Por eso el inyector tiene que abrir la partida **dentro** del proceso del
//! cliente grafico, llamando por dentro al motor.
//!
//! # Como se ha deducido la llamada (resumen; el detalle esta en el ADR-004)
//!
//! Del `.symtab` del motor (no esta stripeado) se resolvieron por nombre
//! mangled:
//!
//!   `_ZN10AppManager7processEv`                                 AppManager::process()
//!   `_ZN10AppManager14createTutorialEv`                         AppManager::createTutorial()
//!   `_ZN10AppManager10createGameERK24ScenarioExecutionContext`  AppManager::createGame()
//!   `_ZN16ScenarioLocationC2E...basic_string...ES7_S7_`         ScenarioLocation::ScenarioLocation()
//!   `_ZN24ScenarioExecutionContextC2ERK16ScenarioLocation10Difficulty9NamedBoolI15EnableReplayTagE`
//!                                                                 ScenarioExecutionContext::ScenarioExecutionContext()
//!
//! Y se dedujo el ABI con `llvm-objdump` (ver el ADR-004):
//!   * `std::__cxx11::basic_string` de libstdc++ son 32 bytes
//!     `{char* p; size_t len; char buf[16];}` con SSO (caben 15 chars).
//!   * `ScenarioLocation` ocupa 0x108 bytes (la dificultad se guarda en el
//!     offset 0x108 del contexto, y la localizacion se copia en el offset 0).
//!   * `ScenarioExecutionContext` se copia a si mismo dentro de
//!     `AppManager::createGame`, asi que se puede pasar uno temporal en la pila.
//!
//! # Cuando se dispara
//!
//! Se engancha `AppManager::process()` (la maquina de estados de la
//! aplicacion) para:
//!   1. capturar el puntero `this` del `AppManager`, y
//!   2. disparar el arranque desde el **principio** del tick, antes de que se
//!      procese el estado actual: asi `createGame` puede empujar su nuevo
//!      estado sin destruir nada que este en ejecucion.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::OnceLock;

use crate::fase_juego;
use crate::log;
use crate::resolver::Simbolos;

// ---------------------------------------------------------------------------
// Simbolos del motor (nombres mangled: no hay ambiguedad con sobrecargas)
// ---------------------------------------------------------------------------

/// `AppManager::process()`.
const MANGLE_APP_PROCESS: &str = "_ZN10AppManager7processEv";
/// `AppManager::createTutorial()`.
const MANGLE_APP_CREATE_TUTORIAL: &str = "_ZN10AppManager14createTutorialEv";
/// `AppManager::createGame(ScenarioExecutionContext const&)`.
const MANGLE_APP_CREATE_GAME: &str = "_ZN10AppManager10createGameERK24ScenarioExecutionContext";
/// `AppManager::popState()`.
const MANGLE_APP_POP_STATE: &str = "_ZN10AppManager8popStateEv";
/// `AppManager::popStateInternal()`.
///
/// Se usa esta y no `popState()`: `popState()` ademas manipula el GUI
/// (`agui::Gui::add`/`TopContainer::remove`) y eso revienta cuando se llama
/// desde fuera del bucle de la aplicacion. `popStateInternal()` es el mismo
///.pop sin la parte del GUI.
const MANGLE_APP_POP_STATE_INTERNAL: &str = "_ZN10AppManager16popStateInternalEv";
/// `AppManager::pushState(AppManagerState*)`.
const MANGLE_APP_PUSH_STATE: &str = "_ZN10AppManager9pushStateEP15AppManagerState";

/// `AppManager::startRconIfEnabled()`.
///
/// Es la **unica** via que tiene el motor para abrir el RCON: la llama el
/// `InLoadingGameBox::process()` (AppManagerStates.cpp:1659) cuando la carga
/// ha terminado. Como la caja de carga se autodestruye en cuanto el estado de
/// la cima no es un `InGame` (ver ADR-004 seccion 12), el RCON nunca llega a
/// abrirse; llamandolo desde aqui se abre igual.
const MANGLE_APP_START_RCON: &str = "_ZN10AppManager18startRconIfEnabledEv";
/// `InLoadingGameBox::InLoadingGameBox(string const&, Filesystem::Path const&, LoadType, bool, NamedBool<DoSpaceAgeFixUpTag>)`.
///
/// Es el estado que usa el propio juego para CARGAR UNA PARTIDA GUARDADA (el
/// boton "Continue Game" del menu). Es el unico camino que abre el RCON:
/// `AppManager::startRconIfEnabled()` se llama desde
/// `InLoadingGameBox::process()`.
const MANGLE_IN_LOADING_GAME_BOX: &str = "_ZN16InLoadingGameBoxC2ERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEERKN10Filesystem4PathE8LoadTypeb9NamedBoolI18DoSpaceAgeFixUpTagE";
/// `Filesystem::Path::Path()` (constructor por defecto).
const MANGLE_FS_PATH_CTOR: &str = "_ZN10Filesystem4PathC2Ev";
/// `operator new(unsigned long)`.
const MANGLE_OP_NEW: &str = "_Znwm";
/// `ScenarioLocation::ScenarioLocation(string const&, string const&, string const&)`.
const MANGLE_SL_CTOR: &str =
    "_ZN16ScenarioLocationC2ERKNSt7__cxx1112basic_stringIcSt11char_traitsIcESaIcEEES7_S7_";
/// `ScenarioExecutionContext::ScenarioExecutionContext(ScenarioLocation const&, Difficulty, NamedBool<EnableReplayTag>)`.
const MANGLE_SEC_CTOR: &str =
    "_ZN24ScenarioExecutionContextC2ERK16ScenarioLocation10Difficulty9NamedBoolI15EnableReplayTagE";
/// `_ZTV6InGame`: la tabla virtual de la clase `InGame`.
///
/// El puntero a la vtable que lleva el objeto es `_ZTV6InGame + 16` (los
/// primeros 16 bytes de la tabla son el "offset to top" y el typeinfo).
/// `InGame` es el estado de "estoy en una partida" y su tipo (`getStateType()`)
/// es 4: el motor usa ese tipo para saber que la carga va en serio.
const MANGLE_VTABLE_IN_GAME: &str = "_ZTV6InGame";
/// Desplazamiento del puntero a la vtable dentro de la tabla virtual.
const OFFSET_VTABLE_OBJETO: usize = 16;

// ---------------------------------------------------------------------------
// Tamanos deducidos del desensamblado
// ---------------------------------------------------------------------------

/// Tamano de `ScenarioLocation`: la dificultad se guarda en el offset 0x108
/// del contexto y la localizacion se copia en su offset 0, luego la localizacion
/// ocupa como mucho 0x108 bytes.
const TAM_SCENARIO_LOCATION: usize = 0x108;

/// Tamano reservado para `ScenarioExecutionContext`. El estado `InCreatingGame`
/// que crea `createGame` mide 0x710 bytes en total (incluye el contexto), asi
/// que 0x800 es holgado.
const TAM_SCENARIO_EXECUTION_CONTEXT: usize = 0x800;

/// `std::__cxx11::basic_string<char>` de libstdc++ (layout con SSO).
///
/// Desdeducido de `ScenarioExecutionContext::getDefault()`, que construye una
/// cadena en la pila asi:
///
/// ```text
///   str  x20, [x19]        ; _M_p = &buf[0]   (buf en +0x10)
///   str  x8,  [x19, #8]    ; _M_string_length
///   str  w9,  [x19, #0x10] ; primeros bytes del texto + NUL
/// ```
#[repr(C)]
pub struct StdString {
    /// Puntero al texto (dentro de `buf` si cabe por SSO).
    p: *mut u8,
    /// Longitud en bytes.
    len: usize,
    /// Buffer local para textos de 15 bytes o menos.
    buf: [u8; 16],
}

/// Buffer alineado para un objeto del motor (los `std::filesystem::path`
/// necesitan al menos 8 bytes de alineacion).
#[repr(C, align(16))]
struct Aligned<const N: usize>([u8; N]);

// ---------------------------------------------------------------------------
// Firmas de las funciones del motor
// ---------------------------------------------------------------------------

/// `AppManager::process()`: `this` en x0, devuelve un `bool`.
///
/// OJO: el mangling Itanium NO codifica el tipo de retorno, asi que el
/// nombre des-mangled ("AppManager::process()") no lo dice. Se dedujo del
/// desensamblado: su unico llamador (MainLoop::prePrepare) hace
/// `bl AppManager::process()` seguido de `cbnz w0, ...`, o sea que el valor
/// de retorno decide si se sigue con la preparacion. Si el hook no devuelve
/// ese valor, el `prePrepare` se corta antes de tiempo y la partida nunca
/// llega a empezar.
type FnThisBool = unsafe extern "C" fn(*mut c_void) -> bool;
/// `AppManager::createTutorial()`: `this` en x0, sin retorno.
type FnThisVoid = unsafe extern "C" fn(*mut c_void);
/// `AppManager::createGame(ScenarioExecutionContext const&)`: x0=this, x1=ctx.
type FnCreateGame = unsafe extern "C" fn(*mut c_void, *const c_void);
/// `ScenarioLocation::ScenarioLocation(s1, s2, s3)`: x0=this, x1..x3=strings.
type FnScenarioLocationCtor =
    unsafe extern "C" fn(*mut c_void, *const c_void, *const c_void, *const c_void);
/// `ScenarioExecutionContext::ScenarioExecutionContext(loc, dificultad, replay)`.
type FnScenarioContextCtor = unsafe extern "C" fn(*mut c_void, *const c_void, u32, u64);

// ---------------------------------------------------------------------------
// Estado global
// ---------------------------------------------------------------------------

/// Direcciones de las funciones del motor ya resueltas por nombre.
static F_CREATE_TUTORIAL: AtomicU64 = AtomicU64::new(0);
static F_CREATE_GAME: AtomicU64 = AtomicU64::new(0);
static F_CTOR_SL: AtomicU64 = AtomicU64::new(0);
static F_CTOR_SEC: AtomicU64 = AtomicU64::new(0);
static F_POP_STATE: AtomicU64 = AtomicU64::new(0);
static F_POP_STATE_INTERNAL: AtomicU64 = AtomicU64::new(0);
static F_PUSH_STATE: AtomicU64 = AtomicU64::new(0);
static F_LOADING_GAME_BOX: AtomicU64 = AtomicU64::new(0);
static F_FS_PATH_CTOR: AtomicU64 = AtomicU64::new(0);
static F_OP_NEW: AtomicU64 = AtomicU64::new(0);
/// `AppManager::startRconIfEnabled()`: la unica via para abrir el RCON.
static F_START_RCON: AtomicU64 = AtomicU64::new(0);
/// Puntero a la vtable del objeto `InGame` (`_ZTV6InGame + 16`).
static VTABLE_IN_GAME: AtomicU64 = AtomicU64::new(0);
/// Puntero al estado `InLoadingGameBox` vivo (el que carga el mapa).
static ESTADO_CARGA: AtomicUsize = AtomicUsize::new(0);
/// Contador de ticks de la fase de carga (diagnostico).
static TICKS_CARGA: AtomicU64 = AtomicU64::new(0);

/// Puntero `this` del `AppManager` (0 hasta que se captura).
static APP_MANAGER: AtomicUsize = AtomicUsize::new(0);
/// 1 cuando ya se ha disparado el arranque.
static DISPARADO: AtomicBool = AtomicBool::new(false);
/// 1 cuando ya se ha llamado a `AppManager::startRconIfEnabled()`.
static RCON_ABIERTO: AtomicBool = AtomicBool::new(false);
/// Numero de intentos de abrir el RCON.
static RCON_INTENTOS: AtomicU64 = AtomicU64::new(0);
/// Numero de ticks de `AppManager::process()` vistos.
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Milisegundos (reloj del sistema) del primer tick.
static INICIO_MS: AtomicU64 = AtomicU64::new(0);

// ---------------------------------------------------------------------------
// Que hay que arrancar
// ---------------------------------------------------------------------------

/// Descripcion de la partida que hay que abrir.
#[derive(Clone, Debug)]
pub enum Plan {
    /// No hay nada que arrancar.
    Nada,
    /// Abrir el tutorial (`AppManager::createTutorial()`).
    Tutorial,
    /// Abrir un escenario (`AppManager::createGame()` con un contexto construido).
    Escenario {
        /// Modulo al que pertenece el escenario ("base", ...).
        modulo: String,
        /// Carpeta de la campana dentro del modulo ("tutorial", "tight-spot"...).
        campana: String,
        /// Nivel dentro de la campana ("level-01", ...).
        nivel: String,
    },
    /// Abrir una partida guardada. **No implementado todavia**: la carga de un
    /// save necesita `Package::openPackage` + `MapDeserialiser` +
    /// `ParallelScenarioLoader::load`, que es otra cadena (ver el ADR-004).
    Guardado {
        /// Ruta del fichero .zip.
        ruta: String,
    },
}

/// Ultimo componente de una ruta con separadores `/`.
fn ultima_parte(ruta: &str) -> String {
    match ruta.rsplit('/').next() {
        Some(p) if !p.is_empty() => p.to_string(),
        _ => ruta.to_string(),
    }
}

/// Lee el plan del entorno.
///
/// Formato de `XFACTORVR_AUTOSTART`:
///   * `tutorial`          -> llama a `AppManager::createTutorial()` (ver las
///     notas de `Plan::Tutorial`: solo funciona si el GUI del tutorial ya
///     esta montado).
///   * `<campana>/<nivel>` -> abre un nivel de campana del modulo `base`
///     (por ejemplo `tutorial/level-01`).
///   * `<modulo>|<campana>|<nivel>` -> igual, pero con el modulo explicito
///     (por ejemplo `base|tutorial|level-01`).
///   * `<ruta>.zip`        -> abriria una partida guardada (sin implementar).
///
/// Se puede afinar el modulo con `XFACTORVR_AUTOSTART_MOD`.
pub fn leer_plan() -> Plan {
    let bruto = match std::env::var("XFACTORVR_AUTOSTART") {
        Ok(v) => v.trim().to_string(),
        Err(_) => return Plan::Nada,
    };
    if bruto.is_empty() {
        return Plan::Nada;
    }

    if bruto.eq_ignore_ascii_case("tutorial") {
        return Plan::Tutorial;
    }

    if bruto.ends_with(".zip") {
        return Plan::Guardado { ruta: bruto };
    }

    // Forma explicita: modulo|campana|nivel (o campana|nivel).
    if bruto.contains('|') {
        let partes: Vec<&str> = bruto.split('|').collect();
        let (modulo, campana, nivel) = match partes.len() {
            3 => (partes[0].to_string(), partes[1].to_string(), partes[2].to_string()),
            2 => ("base".to_string(), partes[0].to_string(), partes[1].to_string()),
            _ => ("base".to_string(), partes[0].to_string(), String::new()),
        };
        return Plan::Escenario { modulo, campana, nivel };
    }

    // Forma corta: campana/nivel.
    if let Some((campana, nivel)) = bruto.split_once('/') {
        return Plan::Escenario {
            modulo: "base".to_string(),
            campana: campana.to_string(),
            nivel: nivel.to_string(),
        };
    }

    Plan::Escenario {
        modulo: "base".to_string(),
        campana: bruto,
        nivel: String::new(),
    }
}

/// Segundos que hay que esperar antes de disparar el arranque.
fn retardo_segundos() -> u64 {
    match std::env::var("XFACTORVR_AUTOSTART_DELAY_S") {
        Ok(v) => (v.trim().parse::<f64>().unwrap_or(3.0).max(0.0) * 1000.0) as u64,
        Err(_) => 3000,
    }
}

/// Dificultad para el contexto (0..3). El propio motor usa 1 en
/// `ScenarioExecutionContext::getDefault()`.
fn dificultad() -> u32 {
    match std::env::var("XFACTORVR_AUTOSTART_DIFFICULTY") {
        Ok(v) => v.trim().parse::<u32>().unwrap_or(1),
        Err(_) => 1,
    }
}

// ---------------------------------------------------------------------------
// Resolucion de simbolos
// ---------------------------------------------------------------------------

/// Resuelve del symtab las funciones del motor que hacen falta.
pub fn resolver_funciones(simbolos: &Simbolos) {
    let mut lineas = String::new();

    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_PROCESS) {
        lineas.push_str(&format!("process=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("process=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_CREATE_TUTORIAL) {
        F_CREATE_TUTORIAL.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("createTutorial=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("createTutorial=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_CREATE_GAME) {
        F_CREATE_GAME.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("createGame=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("createGame=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_SL_CTOR) {
        F_CTOR_SL.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("ctorScenarioLocation=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("ctorScenarioLocation=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_SEC_CTOR) {
        F_CTOR_SEC.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("ctorScenarioExecutionContext=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("ctorScenarioExecutionContext=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_POP_STATE) {
        F_POP_STATE.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("popState=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("popState=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_POP_STATE_INTERNAL) {
        F_POP_STATE_INTERNAL.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("popStateInternal=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("popStateInternal=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_PUSH_STATE) {
        F_PUSH_STATE.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("pushState=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("pushState=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_IN_LOADING_GAME_BOX) {
        F_LOADING_GAME_BOX.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("InLoadingGameBox=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("InLoadingGameBox=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_FS_PATH_CTOR) {
        F_FS_PATH_CTOR.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("FilesystemPath=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("FilesystemPath=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_OP_NEW) {
        F_OP_NEW.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("operatorNew=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("operatorNew=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_VTABLE_IN_GAME) {
        VTABLE_IN_GAME.store(r.direccion + OFFSET_VTABLE_OBJETO as u64, Ordering::SeqCst);
        lineas.push_str(&format!("vtableInGame=0x{:x} ", r.direccion + OFFSET_VTABLE_OBJETO as u64));
    } else {
        lineas.push_str("vtableInGame=NO ");
    }
    if let Some(r) = simbolos.resolver_exacto(MANGLE_APP_START_RCON) {
        F_START_RCON.store(r.direccion, Ordering::SeqCst);
        lineas.push_str(&format!("startRcon=0x{:x} ", r.direccion));
    } else {
        lineas.push_str("startRcon=NO ");
    }
    log::info(&format!("funciones del motor para el autostart: {lineas}"));
}

// ---------------------------------------------------------------------------
// Pila de estados del AppManager
// ---------------------------------------------------------------------------

/// Volca la estructura del AppManager y de su pila de estados (diagnostico).
fn volcar_app_manager(app_manager: *mut c_void) {
    // SAFETY: lectura pura de memoria del proceso, con los offsets deducidos.
    unsafe {
        let app = app_manager as *const u8;
        log::info("--- volcado del AppManager ---");
        for off in [0x250usize, 0x258, 0x260, 0x268] {
            let v: u64 = *(app.add(off) as *const u64);
            log::info(&format!("  app+0x{off:03x} = 0x{v:016x}"));
        }
        let begin: *const u64 = *(app.add(0x258) as *const *const u64);
        let end: *const u64 = *(app.add(0x260) as *const *const u64);
        log::info(&format!(
            "  pila: begin=0x{:x} end=0x{:x} ({} entradas)",
            begin as usize,
            end as usize,
            (end as usize - begin as usize) / 8
        ));
        let mut i = 0usize;
        let mut p = begin;
        while p < end && i < 8 {
            let estado: u64 = *p;
            log::info(&format!("  [{i}] estado = 0x{estado:016x}"));
            if estado != 0 {
                let f0: u64 = *(estado as *const u64);
                let f1: u64 = *((estado + 8) as *const u64);
                let f2: u64 = *((estado + 16) as *const u64);
                log::info(&format!("      [0]=0x{f0:016x} [1]=0x{f1:016x} [2]=0x{f2:016x}"));
                if f0 != 0 {
                    let g0: u64 = *(f0 as *const u64);
                    let g8: u64 = *((f0 + 0x40) as *const u64);
                    log::info(&format!("      vtable[0]=0x{g0:016x} vtable[8]=0x{g8:016x}"));
                    // getStateType() esta en el slot 0x40 de la tabla virtual.
                    let g: unsafe extern "C" fn(*mut c_void) -> u32 =
                        std::mem::transmute(g8 as usize);
                    let t = g(estado as *mut c_void);
                    log::info(&format!("      tipo de estado = {t}"));
                }
            }
            p = p.add(1);
            i += 1;
        }
        log::info("--- fin del volcado ---");
    }
}

/// Devuelve el tipo del estado que esta en lo alto de la pila del AppManager
/// (0 si la pila esta vacia).
///
/// La pila de estados son dos punteros en el AppManager: `begin` en el
/// offset 0x258 y `end` en el 0x260. El tipo de un estado se saca de la tabla
/// virtual, en el slot 0x40 (`getStateType()`).
fn tipo_estado_cima(app_manager: *mut c_void) -> u32 {
    // SAFETY: se leen dos punteros del objeto AppManager en los offsets
    // conocidos (0x258 y 0x260) y se llama a la funcion virtual del slot 0x40.
    unsafe {
        let app = app_manager as *const u8;
        // El AppManager guarda dos punteros: `begin` en 0x258 y `end` en 0x260.
        // Son los extremos del vector de estados; el estado de la cima (el
        // ultimo) es `*(end - 1)`, igual que hace `AppManager::isAppInMenu()`.
        let begin: *const u64 = *(app.add(0x258) as *const *const u64);
        let end: *const u64 = *(app.add(0x260) as *const *const u64);
        if begin.is_null() || end.is_null() || begin >= end {
            return 0;
        }
        let estado: *const u8 = *end.sub(1) as *const u8;
        if estado.is_null() {
            return 0;
        }
        // El primer campo del estado es su tabla virtual.
        let vtable: *const u8 = *(estado as *const *const u8);
        if vtable.is_null() {
            return 0;
        }
        let f: *const u8 = *(vtable.add(0x40) as *const *const u8);
        if f.is_null() {
            return 0;
        }
        let g: unsafe extern "C" fn(*mut c_void) -> u32 = std::mem::transmute(f);
        g(estado as *mut c_void)
    }
}

/// Numero de estados que hay en la pila del AppManager.
fn numero_estados(app_manager: *mut c_void) -> usize {
    // SAFETY: lectura de los dos punteros de la pila de estados.
    unsafe {
        let app = app_manager as *const u8;
        let begin: *const u64 = *(app.add(0x258) as *const *const u64);
        let end: *const u64 = *(app.add(0x260) as *const *const u64);
        if begin.is_null() || end.is_null() || begin >= end {
            return 0;
        }
        (end as usize - begin as usize) / 8
    }
}

/// Saca un estado de la pila (`AppManager::popStateInternal()`), que saca el de
/// la cima.
fn sacar_un_estado(app_manager: *mut c_void) -> bool {
    let pop = F_POP_STATE_INTERNAL.load(Ordering::SeqCst);
    if pop == 0 || numero_estados(app_manager) == 0 {
        return false;
    }
    // SAFETY: la direccion viene del symtab del motor; `popStateInternal()`
    // solo necesita el `this`.
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void) = std::mem::transmute(pop);
        f(app_manager);
    }
    true
}

/// `Filesystem::Path` del motor: un `std::filesystem::path` de libstdc++.
///
/// Layout deducido de `Filesystem::Path::Path()`:
/// ```text
///   +0x00  std::string _M_pathname   (32 bytes: p, len, buf[16])
///   +0x20  path::_List   _M_cmpts    (8 bytes)
/// ```
#[repr(C)]
pub struct FilesystemPath {
    /// Nombre del fichero (un `std::string`).
    pathname: StdString,
    /// Lista de componentes (un `path::_List`, la inicializa el constructor).
    cmpts: *mut u8,
}

/// Construye un `Filesystem::Path` con el texto dado.
///
/// Se llama al constructor real del motor (`Filesystem::Path::Path()`) para que
/// la lista de componentes quede bien, y luego se escribe el texto en el
/// `std::string` interno. El `std::filesystem::path` de libstdc++ recalcula
/// los componentes a partir del texto cuando hace falta (`_M_split_cmpts()`),
/// asi que escribir el texto directamente es valido.
fn construir_filesystem_path(texto: &str) -> Option<Box<FilesystemPath>> {
    let ctor = F_FS_PATH_CTOR.load(Ordering::SeqCst);
    let op_new = F_OP_NEW.load(Ordering::SeqCst);
    if ctor == 0 {
        log::error("no esta resuelto Filesystem::Path::Path()");
        return None;
    }
    let mut p = Box::new(FilesystemPath {
        pathname: StdString { p: std::ptr::null_mut(), len: 0, buf: [0u8; 16] },
        cmpts: std::ptr::null_mut(),
    });
    // SAFETY: `p` tiene el layout de `Filesystem::Path` y el constructor real
    // del motor solo lo inicializa.
    unsafe {
        let f: unsafe extern "C" fn(*mut c_void) = std::mem::transmute(ctor);
        f(&mut *p as *mut FilesystemPath as *mut c_void);
    }
    let bytes = texto.as_bytes();
    if bytes.len() > 15 {
        // El texto no cabe en el SSO: hay que pedir memoria al motor para que
        // la destruccion posterior use su `operator delete`.
        if op_new == 0 {
            log::error("no esta resuelto operator new()");
            return None;
        }
        // SAFETY: reservamos `len + 1` bytes con el allocator del motor.
        let buf = unsafe {
            let f: unsafe extern "C" fn(usize) -> *mut c_void = std::mem::transmute(op_new);
            f(bytes.len() + 1)
        };
        if buf.is_null() {
            log::error("operator new() devolvio null");
            return None;
        }
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf as *mut u8, bytes.len());
            *((buf as *mut u8).add(bytes.len())) = 0;
        }
        p.pathname.p = buf as *mut u8;
        p.pathname.len = bytes.len();
    } else {
        escribir_std_string(&mut p.pathname, texto);
    }
    Some(p)
}

/// Carga una partida guardada, igual que hace el boton "Continue Game" del
/// menu.
///
/// El motor lo hace asi (seccion `InMainMenu::process` del desensamblado):
///
/// ```text
///   auto* box = new InLoadingGameBox(nombre, rutaDelSave, LoadType, false,
///                                     NamedBool<DoSpaceAgeFixUpTag>(false));
///   appManager->pushState(box);
/// ```
///
/// `InLoadingGameBox` es el estado que se encarga de cargar el save (y abre el
/// RCON con `AppManager::startRconIfEnabled()`).
fn arrancar_guardado(app_manager: *mut c_void, ruta: &str) -> bool {
    let ctor = F_LOADING_GAME_BOX.load(Ordering::SeqCst);
    let push = F_PUSH_STATE.load(Ordering::SeqCst);
    let op_new = F_OP_NEW.load(Ordering::SeqCst);
    if ctor == 0 || push == 0 || op_new == 0 {
        log::error("no estan resueltas las funciones para cargar una partida guardada");
        return false;
    }

    // El nombre que usa el motor es el del save (sin extension).
    let nombre = ultima_parte(ruta);
    let mut cadena = Box::new(StdString { p: std::ptr::null_mut(), len: 0, buf: [0u8; 16] });
    escribir_std_string(&mut cadena, &nombre);

    let path = match construir_filesystem_path(ruta) {
        Some(p) => p,
        None => return false,
    };

    // El estado mide 0x60 bytes (lo reserva el propio motor con operator new).
    let objeto = unsafe {
        let f: unsafe extern "C" fn(usize) -> *mut c_void = std::mem::transmute(op_new);
        f(0x60)
    };
    if objeto.is_null() {
        log::error("operator new() devolvio null");
        return false;
    }

    log::info(&format!("cargando la partida guardada '{ruta}'"));

    // SAFETY: la direccion viene del symtab del motor. Los argumentos siguen
    // el ABI deducido del desensamblado:
    //   x0 = this, x1 = &nombre, x2 = &ruta, w3 = LoadType, w4 = bool, x5 = tag.
    unsafe {
        let f_ctor: unsafe extern "C" fn(
            *mut c_void,
            *const c_void,
            *const c_void,
            u32,
            u32,
            u64,
        ) = std::mem::transmute(ctor);
        f_ctor(
            objeto,
            &*cadena as *const StdString as *const c_void,
            &*path as *const FilesystemPath as *const c_void,
            0, // LoadType::loadMap (0), el mismo que usa el boton "Continue Game"
            0, // bool
            0, // NamedBool<DoSpaceAgeFixUpTag>
        );

        let f_push: unsafe extern "C" fn(*mut c_void, *mut c_void) = std::mem::transmute(push);
        f_push(app_manager, objeto);
    }

    // Encima del `InLoadingGameBox` hay que poner un estado de tipo `InGame`
    // (4). Motivo (ver modulo `fase_juego`): `InLoadingGameBox::process()`
    // comprueba el tipo del estado de la CIMA y, si no es `InGame`, da la
    // carga por perdida (`popStateToMainMenu()` + `pushState(InGame)`), lo
    // que destruye este mismo estado y con el el hilo del cargador: la carga
    // se queda en "Checksum for script __level__/control.lua" para siempre.
    //
    // El motor llama al `process()` del estado de la cima en cada frame, asi
    // que con el `InGame` encima es el `InGame::process()` quien corre, y el
    // inyector llama a mano al `process()` del `InLoadingGameBox` (que es el
    // que consulta el cargador y el que abre el RCON).
    if empujar_estado_in_game_habilitado() {
        if !empujar_estado_in_game(app_manager) {
            log::error("no se pudo empujar el estado InGame encima del InLoadingGameBox");
        }
    }

    // El estado de carga queda apuntado para poder llamar a su `process()`.
    ESTADO_CARGA.store(objeto as usize, Ordering::SeqCst);
    true
}

/// Si hay que empujar el estado `InGame` encima del `InLoadingGameBox`.
///
/// **Por defecto NO**: el `InGame` de 16 bytes que empuja el motor es un
/// objeto a medio construir, y `InGame::process()` lo saca de la pila (hace
/// `popStateToMainMenu()`) cuando no encuentra la vista de control. El motor
/// sigue llamando a su `process()` y el juego peta con un SIGSEGV por usar
/// memoria liberada. Se deja como experimento con
/// `XFACTORVR_ESTADO_INGAME=1`.
fn empujar_estado_in_game_habilitado() -> bool {
    match std::env::var("XFACTORVR_ESTADO_INGAME") {
        Ok(v) => v == "1",
        Err(_) => false,
    }
}

/// Crea y empuja un estado `InGame` (16 bytes: vtable + puntero a null) encima
/// del estado actual.
///
/// Es el mismo objeto que crea el propio motor en
/// `InLoadingGameBox::process()` (`new(16)` con la vtable de `InGame` y un
/// puntero a null): 16 bytes, la vtable en el primer word y un puntero nulo
/// en el segundo.
fn empujar_estado_in_game(app_manager: *mut c_void) -> bool {
    let vtable = VTABLE_IN_GAME.load(Ordering::SeqCst);
    let op_new = F_OP_NEW.load(Ordering::SeqCst);
    let push = F_PUSH_STATE.load(Ordering::SeqCst);
    if vtable == 0 || op_new == 0 || push == 0 {
        return false;
    }
    // SAFETY: `operator new` del motor (mismo allocator que usa el motor para
    // los estados, asi que el destructor tambien usa el de freed del motor).
    let objeto = unsafe {
        let f: unsafe extern "C" fn(usize) -> *mut c_void = std::mem::transmute(op_new);
        f(16)
    };
    if objeto.is_null() {
        return false;
    }
    // SAFETY: el objeto tiene 16 bytes: vtable en +0 y puntero nulo en +8.
    unsafe {
        let p = objeto as *mut u64;
        p.write(vtable);
        p.add(1).write(0);
        let f_push: unsafe extern "C" fn(*mut c_void, *mut c_void) = std::mem::transmute(push);
        f_push(app_manager, objeto);
    }
    true
}

/// Saca del AppManager el estado del menu principal.
///
/// Es IMPRESCINDIBLE: `AppManager::createGame()` solo saca de la pila los
/// estados de tipo 4, 5, 6 y 8 (los usa para limpiar cargas en curso). El
/// menu principal es de tipo 2, asi que se queda en la pila y el bucle de
/// estados de `MainLoop::gameUpdateStep()` (que va de abajo arriba y se
/// para en el primer estado que devuelve "true") se queda con el y nunca
/// llega a `InCreatingGame::process()`, que es el que acaba empujando el
/// estado de la partida.
///
/// En el GUI de verdad esto no pasa porque la pantalla "New Game" ya habia
/// sacado el menu antes de llamar a `createGame()`.
fn sacar_menu(app_manager: *mut c_void) {
    if debug() {
        volcar_app_manager(app_manager);
    }
    // Se dejan los estados de encima y se queda solo el de abajo (el menu
    // principal), que es el estado base que el motor da por hecho que
    // existe: su `AppManager::popState()` lee `*(fin - 1)` y con la pila
    // vacia leeria memoria basura -> SIGSEGV en `changeStateInternal()`.
    //
    // Se usa `popStateInternal()` y no `popState()` porque `popState()` ademas
    // mueve el widget del estado entre contenedores del GUI y eso revienta
    // cuando se llama desde fuera del bucle de la aplicacion.
    let mut n = numero_estados(app_manager);
    while n > 1 {
        if !sacar_un_estado(app_manager) {
            break;
        }
        n = numero_estados(app_manager);
    }
    log::info(&format!(
        "pila de estados tras dejar solo el base: {} estados",
        numero_estados(app_manager)
    ));
    let tipo = tipo_estado_cima(app_manager);
    log::info(&format!("tipo del estado que queda: {tipo}"));
}

/// Diagnostico extra del estado del AppManager (`XFACTORVR_AUTOSTART_DEBUG=1`).
fn debug() -> bool {
    matches!(std::env::var("XFACTORVR_AUTOSTART_DEBUG"), Ok(v) if v == "1")
}

// ---------------------------------------------------------------------------
// Construccion de los argumentos
// ---------------------------------------------------------------------------

/// Escribe un `std::string` de libstdc++ en `destino`.
///
/// Solo se soportan textos de 15 bytes o menos (SSO): si el texto es mas
/// largo, se trunca. Es suficiente para los nombres de modulo, ruta y
/// escenario de la campana.
fn escribir_std_string(destino: &mut StdString, texto: &str) {
    let bytes = texto.as_bytes();
    let len = bytes.len().min(15);
    destino.len = len;
    destino.buf = [0u8; 16];
    if len > 0 {
        // SAFETY: `len <= 15` y `buf` tiene 16 bytes.
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), destino.buf.as_mut_ptr(), len);
        }
    }
    destino.buf[len] = 0;
    let p = destino.buf.as_mut_ptr();
    destino.p = p;
}

/// Construye el `ScenarioLocation` y el `ScenarioExecutionContext` y llama a
/// `AppManager::createGame()`.
///
/// El `ScenarioLocation` se construye con el constructor real del motor:
///
/// ```text
///   ScenarioLocation(modulo, campana, nivel)
/// ```
///
/// y `ScenarioLocation::getPrototypePath()` (deducido del desensamblado)
/// resuelve la carpeta del prototipo como:
///
/// ```text
///   mod.getCampaignsPath() / campana / nivel
///   = data/base/campaigns/<campana>/<nivel>
/// ```
///
/// que es exactamente donde vive un nivel de campana
/// (`data/base/campaigns/tutorial/level-01`, ...).
fn arrancar_escenario(
    app_manager: *mut c_void,
    modulo: &str,
    campana: &str,
    nivel: &str,
) -> bool {
    let ctor_sl = F_CTOR_SL.load(Ordering::SeqCst);
    let ctor_sec = F_CTOR_SEC.load(Ordering::SeqCst);
    let crear_game = F_CREATE_GAME.load(Ordering::SeqCst);
    if ctor_sl == 0 || ctor_sec == 0 || crear_game == 0 {
        log::error("no estan resueltas las funciones para arrancar un escenario");
        return false;
    }

    // Las tres cadenas van en un `Box` para que su direccion sea estable
    // (las cadenas SSO apuntan a su propio buffer interior).
    let mut cadenas: Box<[StdString; 3]> = Box::new([
        StdString { p: std::ptr::null_mut(), len: 0, buf: [0u8; 16] },
        StdString { p: std::ptr::null_mut(), len: 0, buf: [0u8; 16] },
        StdString { p: std::ptr::null_mut(), len: 0, buf: [0u8; 16] },
    ]);
    escribir_std_string(&mut cadenas[0], modulo);
    escribir_std_string(&mut cadenas[1], campana);
    escribir_std_string(&mut cadenas[2], nivel);

    let mut localizacion =
        Box::new(Aligned::<TAM_SCENARIO_LOCATION>([0u8; TAM_SCENARIO_LOCATION]));
    let mut contexto =
        Box::new(Aligned::<TAM_SCENARIO_EXECUTION_CONTEXT>([0u8; TAM_SCENARIO_EXECUTION_CONTEXT]));

    log::info(&format!(
        "arrancando escenario: modulo='{modulo}' campana='{campana}' nivel='{nivel}' dificultad={}",
        dificultad()
    ));

    // Primero se saca el menu (ver `sacar_menu`).
    sacar_menu(app_manager);

    // SAFETY: las direcciones vienen del symtab del propio motor (son las
    // funciones reales) y los argumentos siguen el ABI deducido del
    // desensamblado. Los objetos se construyen con los constructores reales
    // del motor, asi que su contenido es valido.
    unsafe {
        let f_sl: FnScenarioLocationCtor = std::mem::transmute(ctor_sl);
        f_sl(
            localizacion.0.as_mut_ptr() as *mut c_void,
            &cadenas[0] as *const StdString as *const c_void,
            &cadenas[1] as *const StdString as *const c_void,
            &cadenas[2] as *const StdString as *const c_void,
        );

        let f_sec: FnScenarioContextCtor = std::mem::transmute(ctor_sec);
        f_sec(
            contexto.0.as_mut_ptr() as *mut c_void,
            localizacion.0.as_mut_ptr() as *const c_void,
            dificultad(),
            0,
        );

        let f_game: FnCreateGame = std::mem::transmute(crear_game);
        f_game(app_manager, contexto.0.as_mut_ptr() as *const c_void);
    }
    true
}

/// Abre el RCON llamando a `AppManager::startRconIfEnabled()`.
///
/// Es la **UNICA** via que tiene el motor para abrir el RCON: la llama el
/// `InLoadingGameBox::process()` (AppManagerStates.cpp:1659) cuando la carga
/// ha terminado. Pero la caja de carga se autodestruye en cuanto el estado de
/// la cima de la pila no es un `InGame` (ADR-004, seccion 12.2), asi que por
/// la via normal el RCON **nunca** llega a abrirse.
///
/// Se llama **una sola vez**: la funcion crea una interfaz nueva en el
/// `RemoteCommandProcessor`, asi que llamarla dos veces intentaria abrir dos
/// sockets en el mismo puerto.
pub fn abrir_rcon(app_manager: *mut c_void) -> bool {
    let dir = F_START_RCON.load(Ordering::SeqCst);
    if dir == 0 {
        log::error("no esta resuelta AppManager::startRconIfEnabled()");
        return false;
    }
    log::info("abriendo el RCON (AppManager::startRconIfEnabled)");
    // SAFETY: la direccion viene del symtab del motor y la firma es `void(this)`.
    unsafe {
        let f: FnThisVoid = std::mem::transmute(dir);
        f(app_manager);
    }
    true
}

/// Abre el RCON en cuanto la partida esta en marcha, y solo una vez.
///
/// Se llama desde el hook de render, que corre FUERA de
/// `AppManager::process()` (llamar ahi dentro seria reentrante y el juego se
/// SIGSEGV).
///
/// OJO: `startRconIfEnabled()` crea la interfaz del RCON en el
/// `RemoteCommandProcessor`, que **todavia no esta en marcha** en el instante
/// en que el motor empuja el `InGame` (el mapa se crea despues). Por eso se
/// reintenta cada pocos frames: el motor, en su flujo normal, tambien la
/// llama manyas veces (la `InLoadingGameBox::process()` la llama en cada
/// frame mientras la carga esta terminada).
pub fn tick_rcon() {
    let app = APP_MANAGER.load(Ordering::Relaxed) as *mut c_void;
    if app.is_null() {
        return;
    }
    // Solo cuando hay un `InGame` en la cima: si no, la partida no esta en
    // marcha y el RCON no serviria de nada.
    if !crate::diagnostico::es_in_game(app) {
        return;
    }
    let intentos = RCON_INTENTOS.fetch_add(1, Ordering::SeqCst) + 1;
    // Cada 30 frames (el motor va a ~1 fps con el rasterizador por software).
    if intentos % 30 != 0 && intentos > 1 {
        return;
    }
    if intentos > 600 {
        return;
    }
    if abrir_rcon(app) {
        RCON_ABIERTO.store(true, Ordering::SeqCst);
    }
}

/// Llama a `AppManager::createTutorial()`.
fn arrancar_tutorial(app_manager: *mut c_void) -> bool {
    let crear_tutorial = F_CREATE_TUTORIAL.load(Ordering::SeqCst);
    if crear_tutorial == 0 {
        log::error("no esta resuelta AppManager::createTutorial()");
        return false;
    }
    log::info("arrancando el tutorial (AppManager::createTutorial)");
    // SAFETY: la direccion viene del symtab del motor y la firma es `void(this)`.
    unsafe {
        let f: FnThisVoid = std::mem::transmute(crear_tutorial);
        f(app_manager);
    }
    true
}

// ---------------------------------------------------------------------------
// Disparo
// ---------------------------------------------------------------------------

/// Plan de arranque, cacheado: se lee del entorno una sola vez.
static PLAN: OnceLock<Plan> = OnceLock::new();

/// Devuelve el plan de arranque (lo lee del entorno la primera vez).
fn plan_global() -> Option<Plan> {
    let plan = PLAN.get_or_init(|| {
        let p = leer_plan();
        match &p {
            Plan::Nada => log::info("autostart: no configurado (XFACTORVR_AUTOSTART vacio)"),
            Plan::Tutorial => log::info("autostart: tutorial"),
            Plan::Escenario { modulo, campana, nivel } => log::info(&format!(
                "autostart: escenario modulo='{modulo}' campana='{campana}' nivel='{nivel}'"
            )),
            Plan::Guardado { ruta } => log::info(&format!("autostart: partida guardada '{ruta}'")),
        }
        p
    });
    match plan {
        Plan::Nada => None,
        other => Some(other.clone()),
    }
}

/// Milisegundos desde la epoca (reloj del sistema).
fn now_millis() -> u64 {
    match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => d.as_millis() as u64,
        Err(_) => 0,
    }
}

/// Valor con el que se fuerza `AppManager+0x58` ("estado de la app").
///
/// 0 = no se toca (por defecto). 1 = "hay una partida cargandose".
fn estado_app_configurado() -> i32 {
    match std::env::var("XFACTORVR_ESTADO_APP") {
        Ok(v) => v.parse::<i32>().unwrap_or(0),
        Err(_) => 0,
    }
}

/// Avanza la carga de la partida: llama al `process()` del `InLoadingGameBox`.
///
/// Se llama desde el hook de render (`GameRenderer::render`), que corre una
/// vez por frame FUERA de `AppManager::process()`. Es importante que sea
/// fuera: dentro seria reentrante (el motor esta recorriendo la pila de
/// estados) y el juego se SEGSEGV.
///
/// El `InLoadingGameBox::process()` es el estado que:
///
///   * consulta el hilo del cargador (`ParallelScenarioLoader`),
///   * engancha el `Scenario` que se esta cargando (`MainLoop+0x78`), y
///   * llama a `AppManager::startRconIfEnabled()`, que es la UNICA via para
///     abrir el RCON.
///
/// El motor solo llama al `process()` del estado de la CIMA de la pila, y en
/// la cima esta el `InGame` (que se empuja para que el motor no destruya la
/// caja de carga al ver que la cima no es de tipo `InGame`), asi que la caja
/// de carga hay que empujarla a mano.
pub fn tick_carga() {
    let carga = ESTADO_CARGA.load(Ordering::SeqCst);
    if carga == 0 {
        return;
    }
    let app = APP_MANAGER.load(Ordering::Relaxed) as *mut c_void;
    if app.is_null() {
        return;
    }
    let estado = carga as *mut c_void;
    // Diagnostico: estado del cargador y de la pila de estados.
    {
        let ticks = TICKS_CARGA.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks % 120 == 0 || ticks == 1 {
            // El cargador esta en el +0x18 del `InLoadingGameBox`; su estado
            // esta en el +0x78 y el flag de "cargado" en el +0xa8.
            let cargador = unsafe { (estado as *const u8).add(0x18).cast::<u64>().read() };
            let (est_cargador, flag) = if cargador == 0 {
                (0xffff_ffffu32, 0xffff_ffffu32)
            } else {
                unsafe {
                    (
                        (cargador as *const u8).add(0x78).cast::<u32>().read(),
                        (cargador as *const u8).add(0xa8).cast::<u32>().read(),
                    )
                }
            };
            let cima = fase_juego::estado_cima(app);
            log::info(&format!(
                "carga: tick={ticks} estado_cargador={est_cargador} flag={flag} cima_tipo={} estados={} app_estado={}",
                fase_juego::tipo_de(cima),
                numero_estados(app),
                fase_juego::estado_app(app),
            ));
        }
    }
    // Si la caja de carga ya no esta en la pila, la carga se acabo (o
    // fallo): no se llama a un objeto destruido.
    let cima = fase_juego::estado_cima(app);
    if cima.is_null() {
        return;
    }
    let sigue = (cima as usize) != (estado as usize);
    if !sigue {
        // La cima es otra cosa: el `InGame`. La caja de carga esta debajo.
        // Comprobar que sigue en la pila (puede que se haya sacado).
        let inicio = fase_juego::inicio_pila(app);
        let fin = fase_juego::fin_pila(app);
        let mut dentro = false;
        let mut p = inicio;
        while p < fin {
            // SAFETY: `p` va desde el principio hasta el final de la pila.
            if unsafe { *p as usize } == (estado as usize) {
                dentro = true;
                break;
            }
            // SAFETY: `p` avanza de uno en uno dentro de la pila.
            p = unsafe { p.add(1) };
        }
        if !dentro {
            return;
        }
    }
    // El motor llama al `process()` del estado de la CIMA de la pila, y en la
    // fase de carga esa cima es el `InLoadingGameBox`: ya lo llama el solo.
    // Llamarlo tambien desde aqui es REENTRANTE y, peor, su rama de
    // "abandono" hace `AppManager::popState()`, que toca el GUI y revienta
    // si se llama desde fuera del bucle de la aplicacion (ADR-004 7.4).
    // Por defecto NO se llama; queda como experimento con
    // `XFACTORVR_LLAMAR_CARGA=1`.
    if llamar_carga_habilitado() {
        fase_juego::llamar_process(estado, app);
    }
}

/// Si hay que llamar a mano al `process()` del `InLoadingGameBox`.
///
/// **Por defecto NO** (ver `tick_carga`): el motor ya lo llama y la llamada
/// manual acaba en `popState()`, que revienta. `XFACTORVR_LLAMAR_CARGA=1` lo
/// activa.
fn llamar_carga_habilitado() -> bool {
    match std::env::var("XFACTORVR_LLAMAR_CARGA") {
        Ok(v) => v == "1",
        Err(_) => false,
    }
}

/// Hook de `AppManager::process()`.
///
/// Hace dos cosas:
///   1. captura el puntero `this` del `AppManager` (que no se puede obtener
///      de otra forma comoda), y
///   2. dispara el arranque automatico pasado el retardo configurado.
///
/// Se dispara al **principio** del tick, antes de que el motor procese el
/// estado actual: asi `createGame` puede empujar su estado sin destruir el
/// objeto que este en ejecucion.
#[no_mangle]
pub extern "C" fn xfv_hook_app_process(this: *mut c_void) -> bool {
    // Capturar el AppManager (la primera vez).
    if !this.is_null() && APP_MANAGER.load(Ordering::Relaxed) == 0 {
        APP_MANAGER.store(this as usize, Ordering::SeqCst);
        log::info(&format!("AppManager capturado en 0x{:x}", this as usize));
    }

    // Diagnostico de la pila de estados (XFACTORVR_DIAG_PILA=1): solo lee, no
    // toca nada. Sirve para ver en que estado se queda el motor cuando la
    // carga no avanza (ver ADR-004, seccion 12).
    if crate::diagnostico::habilitado() {
        crate::diagnostico::tick(this);
    }

    // Disparar el arranque automatico.
    if !DISPARADO.load(Ordering::Relaxed) {
        let ticks = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
        if ticks == 1 {
            INICIO_MS.store(now_millis(), Ordering::SeqCst);
            log::info("AppManager::process(): primer tick");
        }
        if ticks % 120 == 0 {
            log::info(&format!("AppManager::process(): {ticks} ticks"));
        }
        let retardo = retardo_segundos();
        let transcurrido = now_millis().saturating_sub(INICIO_MS.load(Ordering::Relaxed));
        if transcurrido >= retardo {
            DISPARADO.store(true, Ordering::SeqCst);
            let app = APP_MANAGER.load(Ordering::Relaxed) as *mut c_void;
            if app.is_null() {
                log::error("no se pudo arrancar: no hay puntero de AppManager");
            } else if let Some(plan) = plan_global() {
                // Antes de arrancar hay que dejar la pila de estados con solo
                // el estado base (el menu): el motor da por hecho que existe.
                sacar_menu(app);
                match &plan {
                    Plan::Tutorial => {
                        arrancar_tutorial(app);
                    }
                    Plan::Escenario { modulo, campana, nivel } => {
                        arrancar_escenario(app, modulo, campana, nivel);
                    }
                    Plan::Guardado { ruta } => {
                        arrancar_guardado(app, ruta);
                    }
                    Plan::Nada => {}
                }
            }
        }
    }

    // Llamar al original y devolver SU valor de retorno: el llamador decide
    // con el si sigue con la preparacion (ver `FnThisBool`).
    let tramp = crate::hooks::trampolin_app_process();
    if tramp == 0 {
        return false;
    }
    // SAFETY: el trampolin contiene el bloque original reubicado y salta
    // al resto de la funcion; `this` se pasa intacto.
    let r = unsafe {
        let f: FnThisBool = std::mem::transmute(tramp);
        f(this)
    };

    // La FASE DE LA CARGA se lleva desde el hook de render (ver `tick_carga`):
    // llamar al `process()` de un estado desde dentro de
    // `AppManager::process()` es REENTRANTE y el motor esta iterating la pila
    // de estados en ese momento: con eso el juego se SEGSEGV.
    if DISPARADO.load(Ordering::Relaxed) {
        let carga = ESTADO_CARGA.load(Ordering::SeqCst);
        if carga != 0 {
            let app = APP_MANAGER.load(Ordering::Relaxed) as *mut c_void;
            if !app.is_null() {
                // Valor de `AppManager+0x58` ("estado de la app"). 0 = no
                // tocarlo (el motor se queda en la fase de menu). 1 = "hay
                // una partida cargandose": es lo que `InGame::process()`
                // comprueba antes de crear la vista de control
                // (`MainLoop+0x2c0`).
                let valor = estado_app_configurado();
                if valor != 0 {
                    fase_juego::poner_estado_app(app, valor);
                }
            }
        }
    }

    // El valor de retorno de `AppManager::process()` decide la FASE del bucle
    // principal: `MainLoop::prePrepare()` (su unico llamador) devuelve ese
    // mismo valor, y `MainLoop::run()` usa el retorno de `prePrepare()` para
    // quedarse en la fase de preparacion o entrar en el bucle del juego.
    //
    // ERROR ANTIGUO (ver ADR-004 seccion 9): aqui se forzaba el retorno a
    // "false" porque se creia que al saltar al bucle del juego
    // `AppManager::process()` dejaria de llamarse y la maquina de estados se
    // quedaria parada. FALSO: `prePrepare()` llama a `AppManager::process()`
    // en CADA frame, tanto en la fase de preparacion como en la del juego
    // (MainLoop.cpp:731).
    //
    // El valor se puede forzar con XFACTORVR_FASE=menu (fase de preparacion,
    // el motor se queda pintando el menu) o =natural (el valor del motor).
    if forzar_fase_menu() {
        return false;
    }
    r
}

/// Si hay que forzar la fase de preparacion (el menu).
///
/// `XFACTORVR_FASE=menu` lo fuerza. Por defecto se deja el valor natural del
/// motor, que es el que sabe si la app sigue en el menu o ya esta jugando.
fn forzar_fase_menu() -> bool {
    match std::env::var("XFACTORVR_FASE") {
        Ok(v) => v == "menu",
        Err(_) => false,
    }
}

//! La **fase del bucle principal** y la pila de estados del `AppManager`.
//!
//! Esto es lo que se ha descubierto con `gdb` sobre el motor de Factorio
//! 2.1.22 (ARM64) y que explica por que la partida se quedaba a medio cargar:
//!
//! * El motor llama al `process(AppManager*)` **del estado que esta en lo alto
//!   de la pila** en cada frame, desde `AppManager::process()` (AppManager.cpp:
//!   264). No es un bucle que recorra la pila: es el estado de la cima.
//! * `InLoadingGameBox::process()` (AppManagerStates.cpp:1659) es lo unico que
//!   llama a `AppManager::startRconIfEnabled()` (la unica via para abrir el
//!   RCON) y lo unico que engancha el `Scenario` que se esta cargando
//!   (`MainLoop+0x78`).
//! * Ese `process()` tiene un guardia: comprueba el **tipo del estado de la
//!   cima**; si no es `InGame` (tipo 4) da la carga por perdida y hace
//!   `popStateToMainMenu()` + `pushState(InGame)`, lo que **destruye el
//!   `InLoadingGameBox` y con el el hilo del cargador**: la carga se queda
//!   a medias y el mapa no se crea nunca.
//!
//! O sea: para que la carga termine, mientras carga el mapa el estado de la
//! cima tiene que ser de tipo `InGame` (4) y el `InLoadingGameBox` tiene que
//! seguir vivo **debajo**. Como el motor solo llama al `process()` de la
//! cima, el inyector llama a mano al `process()` del `InLoadingGameBox`.
//!
//! Tabla virtual de `AppManagerState` (deducida del symtab y de las
//! relocalizaciones de `_ZTV10InMainMenu` / `_ZTV16InLoadingGameBox`):
//!
//! ```text
//!   +0x00  destructor          +0x50  processAction(const AppAction&)
//!   +0x08  destructor          +0x58  processEvent(const Event&)
//!   +0x10  destroy()           +0x60  isListeningToEvent() const
//!   +0x18  getGui() const      +0x68  isSavingMap() const
//!   +0x20  getGui()             +0x70  shouldQueueEvents() const
//!   +0x28  asMultiplayerConnect()   +0x78  isLoadingMap() const
//!   +0x30  asMultiplayerConnect() const
//!   +0x38  getName() const      +0x48  process(AppManager*)   <-- aqui
//!   +0x40  getStateType() const
//! ```

use std::os::raw::c_void;

/// Ranura de `AppManagerState::process(AppManager*)` en la tabla virtual.
pub const RANURA_PROCESS: usize = 0x48;
/// Ranura de `AppManagerState::getStateType() const` en la tabla virtual.
pub const RANURA_TIPO: usize = 0x40;

/// Tipo de estado `InGame` (`InGame::getStateType()` devuelve 4).
pub const TIPO_IN_GAME: u32 = 4;
/// Tipo de estado `InMainMenu` (y todos los menus).
pub const TIPO_MENU: u32 = 2;

/// Offset del principio de la pila de estados dentro del `AppManager`.
pub const PILA_BEGIN: usize = 0x258;
/// Offset del final de la pila de estados dentro del `AppManager`.
pub const PILA_END: usize = 0x260;

/// Offset del campo "estado de la app" dentro del `AppManager`.
///
/// Lo lee `AppManager::process()` para decidir en que fase esta (el menu, la
/// carga, el juego...) y lo comprueba `InGame::process()`: tiene que valer 1
/// ("hay una partida cargandose") para que se cree la vista de control.
pub const APP_ESTADO: usize = 0x58;

/// Devuelve el estado que esta en lo alto de la pila del `AppManager`.
///
/// Devuelve `null` si la pila esta vacia o los punteros no cuadran.
pub fn estado_cima(app_manager: *mut c_void) -> *mut c_void {
    if app_manager.is_null() {
        return std::ptr::null_mut();
    }
    // SAFETY: `app_manager` es el `this` real del motor; los offsets 0x258 y
    // 0x260 son los extremos del vector de estados (ver `numero_estados`).
    unsafe {
        let app = app_manager as *const u8;
        let begin: *const u64 = *(app.add(PILA_BEGIN) as *const *const u64);
        let end: *const u64 = *(app.add(PILA_END) as *const *const u64);
        if begin.is_null() || end.is_null() || begin >= end {
            return std::ptr::null_mut();
        }
        let cima: *const u64 = end.sub(1);
        *cima as *mut c_void
    }
}

/// Devuelve el principio de la pila de estados (puntero a `AppManagerState*`).
pub fn inicio_pila(app_manager: *mut c_void) -> *const u64 {
    if app_manager.is_null() {
        return std::ptr::null();
    }
    // SAFETY: offset 0x258 del `AppManager` = principio del vector de estados.
    unsafe {
        let app = app_manager as *const u8;
        *(app.add(PILA_BEGIN) as *const *const u64)
    }
}

/// Devuelve el final de la pila de estados (uno mas alla del ultimo estado).
pub fn fin_pila(app_manager: *mut c_void) -> *const u64 {
    if app_manager.is_null() {
        return std::ptr::null();
    }
    // SAFETY: offset 0x260 del `AppManager` = final del vector de estados.
    unsafe {
        let app = app_manager as *const u8;
        *(app.add(PILA_END) as *const *const u64)
    }
}

/// Devuelve el tipo (`getStateType()`) del estado dado.
pub fn tipo_de(estado: *mut c_void) -> u32 {
    if estado.is_null() {
        return 0;
    }
    // SAFETY: el primer campo del estado es su tabla virtual; la ranura
    // 0x40 es `getStateType() const` y no toca nada.
    unsafe {
        let vtable: *const u8 = *(estado as *const *const u8);
        if vtable.is_null() {
            return 0;
        }
        let f: *const u8 = *(vtable.add(RANURA_TIPO) as *const *const u8);
        if f.is_null() {
            return 0;
        }
        let g: unsafe extern "C" fn(*mut c_void) -> u32 = std::mem::transmute(f);
        g(estado)
    }
}

/// Llama a `AppManagerState::process(AppManager*)` del estado dado.
///
/// Devuelve el `bool` que devuelve el motor.
pub fn llamar_process(estado: *mut c_void, app_manager: *mut c_void) -> bool {
    if estado.is_null() {
        return false;
    }
    // SAFETY: ranura 0x48 de la tabla virtual = `process(AppManager*)`.
    unsafe {
        let vtable: *const u8 = *(estado as *const *const u8);
        if vtable.is_null() {
            return false;
        }
        let f: *const u8 = *(vtable.add(RANURA_PROCESS) as *const *const u8);
        if f.is_null() {
            return false;
        }
        let g: unsafe extern "C" fn(*mut c_void, *mut c_void) -> bool = std::mem::transmute(f);
        g(estado, app_manager)
    }
}

/// Pone el campo "estado de la app" (`AppManager+0x58`).
///
/// El motor lo usa para saber en que fase esta: con 1 significa "hay una
/// partida cargandose", y es lo que `InGame::process()` comprueba antes de
/// crear la vista de control (`MainLoop+0x2c0`), sin la cual el mapa no se
/// crea y el RCON no abre.
pub fn poner_estado_app(app_manager: *mut c_void, valor: i32) {
    if app_manager.is_null() {
        return;
    }
    // SAFETY: el campo es un `int` en el offset 0x58 del `AppManager`.
    unsafe {
        *(app_manager as *mut u8).add(APP_ESTADO).cast::<i32>() = valor;
    }
}

/// Lee el campo "estado de la app" (`AppManager+0x58`).
pub fn estado_app(app_manager: *mut c_void) -> i32 {
    if app_manager.is_null() {
        return 0;
    }
    // SAFETY: el campo es un `int` en el offset 0x58 del `AppManager`.
    unsafe { (app_manager as *mut u8).add(APP_ESTADO).cast::<i32>().read() }
}

//! Hooks del motor: por ahora solo el de prueba sobre `GameRenderer::render`.
//!
//! El hook de prueba sirve para demostrar la cadena completa:
//!   resolver simbolo -> parchear -> contar frames -> chamar al original.
//!
//! `GameRenderer::render()` es un metodo sin argumentos: recibe `this` en x0
//! y no devuelve nada. El hook tiene que respetar esa convencion (AAPCS64)
//! porque el parche sustituye la funcion entera: si el hook no repasa `this`
//! al trampolin, el motor recibe un `this` invalido y se rompe.

use std::ffi::c_void;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::detour::{self, Estilo, Hook};
use crate::log;
use crate::resolver;

/// Contador de frames que han pasado por el hook.
pub static FRAMES: AtomicU64 = AtomicU64::new(0);

/// Direccion del trampolin (para chamar al original).
static TRAMPOLIN: AtomicUsize = AtomicUsize::new(0);

/// Direccion del trampolin de `AppManager::process()`.
static TRAMPOLIN_APP: AtomicUsize = AtomicUsize::new(0);
/// Trampolin de `MainLoop::processEvents(bool)`.
static TRAMPOLIN_EVENTS: AtomicUsize = AtomicUsize::new(0);

/// Cada cuantos frames se imprime el "pull" de depuracion.
static CADA: AtomicU64 = AtomicU64::new(60);

/// Si el "pull" de depuracion esta activo.
static DEBUG: AtomicU64 = AtomicU64::new(1);

/// Si vale 1, el hook NO llama al original (diagnostico: sirve para saber si
/// el fallo esta en el trampolin o en el hook).
static NOCALL: AtomicU64 = AtomicU64::new(0);

/// Traza de cada entrada al hook (diagnostico).
static TRACE: AtomicU64 = AtomicU64::new(0);

/// Funcion hooked: `GameRenderer::render()`.
///
/// Se exporta con `#[no_mangle]` para poder pedir su direccion REAL de
/// ejecucion con `dlsym`. Tomar el puntero a la funcion (`hook as *const ()`)
/// puede devolver la direccion de ENLAZADO, no la de ejecucion: si el
/// segmento de texto tiene direccion fija (como el de la glibc empaquetada de
/// Termux), el parche saltaria a una direccion inexistente y el juego
/// recibiria un SIGSEGV en el primer frame sin entrar nunca en el hook.
#[no_mangle]
pub extern "C" fn xfv_hook_render(this: *mut c_void) {
    hook_render(this)
}

/// Trampolin de `AppManager::process()` (para llamar al original).
pub fn trampolin_app_process() -> usize {
    TRAMPOLIN_APP.load(Ordering::Relaxed)
}

/// Trampolin de `MainLoop::processEvents(bool)`.
pub fn trampolin_process_events() -> usize {
    TRAMPOLIN_EVENTS.load(Ordering::Relaxed)
}

/// Hook de `MainLoop::processEvents(bool)`.
///
/// Es el punto correcto para avanzar la carga: se llama **una vez por frame**
/// en todas las fases (menu, carga y juego), mientras que
/// `GameRenderer::render` solo se llama cuando se dibuja el juego (y en la
/// fase de carga no se dibuja, asi que ahi no se puede).
#[no_mangle]
pub extern "C" fn xfv_hook_process_events(this: *mut c_void, in_game: u8) -> u8 {
    // Avanzo de la carga de la partida (si la hay). Se hace aqui, y no en el
    // hook de `AppManager::process()`, porque el `process()` de un estado no
    // se puede llamar desde dentro de `AppManager::process()` (es
    // reentrante: el motor esta recorriendo la pila de estados).
    crate::autostart::tick_carga();

    let tramp = TRAMPOLIN_EVENTS.load(Ordering::Relaxed);
    if tramp == 0 {
        return in_game;
    }
    // SAFETY: el trampolin contiene el bloque original reubicado y salta al
    // resto de la funcion; los argumentos se pasan intactos.
    unsafe {
        let f: extern "C" fn(*mut c_void, u8) -> u8 = std::mem::transmute(tramp);
        f(this, in_game)
    }
}

/// Cuerpo del hook.
extern "C" fn hook_render(this: *mut c_void) {
    // Traza minima de la entrada (diagnostico): se escribe con write(2) para
    // no depender de nada mas. Solo con XFACTORVR_HOOK_TRACE=1.
    if TRACE.load(Ordering::Relaxed) == 1 {
        crate::log::escribir_stderr(b"[xfv] hook: entrada\n");
    }
    let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;

    if DEBUG.load(Ordering::Relaxed) == 1 {
        let cada = CADA.load(Ordering::Relaxed).max(1);
        if n % cada == 0 || n == 1 {
            // Sin `format!` para no depender del formateador: basta con
            // escribir el numero a mano en un buffer fijo.
            let mut buf = [0u8; 64];
            let s = escribir_decimal(&mut buf, n);
            let mut linea = [0u8; 96];
            let pre = b"[xfactorvr] hook vivo: ";
            let suf = b" frames\n";
            let mut n1 = 0;
            linea[n1..n1 + pre.len()].copy_from_slice(pre);
            n1 += pre.len();
            linea[n1..n1 + s.len()].copy_from_slice(s.as_bytes());
            n1 += s.len();
            linea[n1..n1 + suf.len()].copy_from_slice(suf);
            n1 += suf.len();
            crate::log::escribir_stderr(&linea[..n1]);
        }
    }

    // Avanzo de la carga de la partida (si la hay). Se hace aqui, y no en el
    // hook de `AppManager::process()`, porque el `process()` de un estado no
    // se puede llamar desde dentro de `AppManager::process()` (es
    // reentrante: el motor esta recorriendo la pila de estados).
    crate::autostart::tick_carga();

    // Abrir el RCON cuando la partida ya esta en marcha (una sola vez).
    crate::autostart::tick_rcon();

    // Llamar al original a traves del trampolin, pasando `this` intacto.
    let tramp = TRAMPOLIN.load(Ordering::Relaxed);
    if tramp != 0 && NOCALL.load(Ordering::Relaxed) == 0 {
        // SAFETY: el trampolin contiene el bloque original reubicado y
        // termina saltando a destino+bytes, que es el resto de la funcion.
        unsafe {
            let f: Hook = std::mem::transmute(tramp);
            f(this);
        }
    }
}

/// Escribe `n` en decimal en `buf` y devuelve el trozo escrito.
fn escribir_decimal(buf: &mut [u8; 64], n: u64) -> &str {
    let mut i = buf.len();
    let mut v = n;
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    // SAFETY: el buffer es de bytes ASCII y solo escribimos digitos.
    unsafe { std::str::from_utf8_unchecked(&buf[i..]) }
}

/// Arranca el inyector: resuelve el simbolo, instala el detour y activa el
/// contador de frames.
pub fn instalar() {
    // --- configuracion por entorno ---
    let debug = match std::env::var("XFACTORVR_HOOK_DEBUG") {
        Ok(v) => v != "0",
        Err(_) => true,
    };
    DEBUG.store(if debug { 1 } else { 0 }, Ordering::Relaxed);

    let cada = match std::env::var("XFACTORVR_HOOK_EVERY") {
        Ok(v) => v.parse::<u64>().unwrap_or(60).max(1),
        Err(_) => 60,
    };
    CADA.store(cada, Ordering::Relaxed);

    let nocall = match std::env::var("XFACTORVR_HOOK_NOCALL") {
        Ok(v) => v != "0",
        Err(_) => false,
    };
    NOCALL.store(if nocall { 1 } else { 0 }, Ordering::Relaxed);

    let trace = match std::env::var("XFACTORVR_HOOK_TRACE") {
        Ok(v) => v != "0",
        Err(_) => false,
    };
    TRACE.store(if trace { 1 } else { 0 }, Ordering::Relaxed);

    let objetivo = match std::env::var("XFACTORVR_HOOK_TARGET") {
        Ok(v) if !v.is_empty() => v,
        _ => "GameRenderer::render".to_string(),
    };

    let estilo = match std::env::var("XFACTORVR_HOOK_PATCH").as_deref() {
        Ok("corto") => Some(Estilo::Corto),
        Ok("largo") => Some(Estilo::Largo),
        _ => None,
    };

    log::info(&format!(
        "arrancando: objetivo='{objetivo}' debug={} cada={cada}",
        if debug { "si" } else { "no" }
    ));

    // --- localizar el ejecutable ---
    let proceso = match resolver::proceso_principal() {
        Some(p) => p,
        None => {
            log::aviso("no se pudo localizar el ejecutable principal: no se instala ningun hook");
            return;
        }
    };
    log::info(&format!(
        "ejecutable: {} base=0x{:x}",
        proceso.ruta.display(),
        proceso.load_bias
    ));

    // --- cargar el symtab ---
    let simbolos = match resolver::Simbolos::cargar(&proceso) {
        Some(s) => s,
        None => {
            log::aviso(
                "el ejecutable no tiene .symtab (esta stripeado): no se puede resolver por nombre",
            );
            return;
        }
    };
    log::info(&format!(
        "simbolos de funcion leidos: {}",
        simbolos.imagen.symbols.len()
    ));

    // --- funciones del motor para el arranque automatico ---
    // Se resuelven siempre (es barato) para poder informar de lo que falta.
    crate::autostart::resolver_funciones(&simbolos);
    crate::diagnostico::resolver_vtables(&simbolos);
    if autostart_activo() || diagnostico_activo() {
        instalar_hook_app_manager(&simbolos);
        instalar_hook_process_events(&simbolos);
    }

    // --- resolver ---
    let encontrado = match simbolos.resolver(&objetivo) {
        Some(r) => r,
        None => {
            log::error(&format!("no se encontro el simbolo '{objetivo}'"));
            // Ayuda: listar los candidatos mas cercanos.
            let pref = objetivo.split("::").next().unwrap_or("");
            let cands = simbolos.buscar(pref);
            if !cands.is_empty() {
                let muestra: Vec<&str> = cands.iter().take(8).map(|s| s.as_str()).collect();
                log::aviso(&format!("similares: {}", muestra.join(", ")));
            }
            return;
        }
    };
    log::info(&format!(
        "resuelto: {} -> 0x{:x} (mangled {})",
        encontrado.legible, encontrado.direccion, encontrado.mangled
    ));

    // --- instalar el detour ---
    // Direccion REAL de ejecucion del hook (ver xfv_hook_render).
    let hook_addr = match direccion_de_ejecucion() {
        Some(a) => a,
        None => {
            log::aviso("dlsym no devolvio el hook; se usa la direccion de enlace (puede fallar)");
            hook_render as *const () as u64
        }
    };
    match detour::instalar(encontrado.direccion, hook_addr, estilo) {
        Ok(d) => {
            TRAMPOLIN.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!(
                "detour instalado: parche de {} bytes ({:?}), trampolin en 0x{:x}",
                d.estilo.bytes(),
                d.estilo,
                d.trampolin
            ));
            log::info(&format!(
                "hook en 0x{:x}, injector en 0x{:x}",
                hook_addr,
                crate::xfactorvr_injector_init as usize
            ));
            log::info("bytes originales de la funcion:");
            log::hexdump(&d.original);
            log::info("bytes del trampolin (original reubicado + salto de vuelta):");
            // SAFETY: el trampolin esta mapeado y es legible.
            let bytes = unsafe {
                std::slice::from_raw_parts(d.trampolin as *const u8, d.original.len() + 16)
            };
            log::hexdump(bytes);
            log::info("hook activo: cada frame pasa por hook_render()");
        }
        Err(e) => {
            log::error(&format!("no se pudo instalar el detour: {e}"));
        }
    }
}

/// Direccion de ejecucion del hook, pedida al cargador con dlsym.
fn direccion_de_ejecucion() -> Option<u64> {
    direccion_de_ejecucion_de("xfv_hook_render")
}

/// Direccion REAL de ejecucion de un simbolo exportado por esta libreria.
///
/// Tomar el puntero a la funcion (`hook as *const ()`) puede devolver la
/// direccion de ENLAZADO, no la de ejecucion: si el segmento de texto tiene
/// direccion fija, el parche saltaria a una direccion inexistente y el juego
/// recibiria un SIGSEGV sin llegar a entrar en el hook.
fn direccion_de_ejecucion_de(nombre: &str) -> Option<u64> {
    extern "C" {
        fn dlsym(handle: *mut core::ffi::c_void, symbol: *const core::ffi::c_char)
            -> *mut core::ffi::c_void;
    }
    const RTLD_DEFAULT: *mut core::ffi::c_void = 0 as *mut core::ffi::c_void;
    // El nombre tiene que ir con NUL final.
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

/// True si hay un arranque automatico configurado.
fn autostart_activo() -> bool {
    match std::env::var("XFACTORVR_AUTOSTART") {
        Ok(v) => !v.trim().is_empty(),
        Err(_) => false,
    }
}

/// Si hay que instalar el hook del `AppManager` solo para el diagnostico.
///
/// El diagnostico de la pila de estados (`XFACTORVR_DIAG_PILA=1`) necesita el
/// `this` del `AppManager`, que solo se puede sacar de `AppManager::process()`.
/// Con el autostart vacio el motor no arranca ninguna partida (el hook solo
/// lee), asi que se puede usar sin tocar nada.
fn diagnostico_activo() -> bool {
    crate::diagnostico::habilitado()
}

/// Instala el detour sobre `AppManager::process()`.
///
/// Instala el detour sobre `MainLoop::processEvents(bool)`.
///
/// Se engancha aqui (y no en el render) porque `processEvents()` se llama una
/// vez por frame en TODAS las fases (menu, carga y juego), mientras que
/// `GameRenderer::render` solo se llama cuando se dibuja el juego.
fn instalar_hook_process_events(simbolos: &resolver::Simbolos) {
    const MANGLE: &str = "_ZN8MainLoop13processEventsEb";
    let encontrado = match simbolos.resolver_exacto(MANGLE) {
        Some(r) => r,
        None => {
            log::error("no se encontro MainLoop::processEvents() en el symtab");
            return;
        }
    };
    log::info(&format!(
        "resuelto: MainLoop::processEvents() -> 0x{:x}",
        encontrado.direccion
    ));
    let hook_addr = match direccion_de_ejecucion_de("xfv_hook_process_events") {
        Some(a) => a,
        None => {
            log::aviso("dlsym no devolvio el hook de processEvents; se usa la direccion de enlace");
            return;
        }
    };
    match detour::instalar(encontrado.direccion, hook_addr, None) {
        Ok(d) => {
            TRAMPOLIN_EVENTS.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!(
                "detour instalado sobre MainLoop::processEvents(): parche de {} bytes, trampolin en 0x{:x}",
                d.estilo.bytes(),
                d.trampolin
            ));
        }
        Err(e) => {
            log::error(&format!("no se pudo instalar el detour de processEvents(): {e}"));
        }
    }
}

/// Se engancha ahi (y no en el render) por dos razones:///   * `process()` recibe el `this` del `AppManager` en x0, que es lo que
///     hace falta para llamar a `createGame`/`createTutorial`; y
///   * el disparo ocurre al principio del tick, antes de que se procese el
///     estado actual, asi que `createGame` puede empujar su estado sin
///     destruir el objeto que este en ejecucion.
fn instalar_hook_app_manager(simbolos: &resolver::Simbolos) {
    const MANGLE: &str = "_ZN10AppManager7processEv";
    let encontrado = match simbolos.resolver_exacto(MANGLE) {
        Some(r) => r,
        None => {
            log::error("no se encontro AppManager::process() en el symtab");
            return;
        }
    };
    log::info(&format!(
        "resuelto: AppManager::process() -> 0x{:x}",
        encontrado.direccion
    ));

    let hook_addr = match direccion_de_ejecucion_de("xfv_hook_app_process") {
        Some(a) => a,
        None => {
            log::aviso("dlsym no devolvio el hook de AppManager; se usa la direccion de enlace");
            return;
        }
    };

    match detour::instalar(encontrado.direccion, hook_addr, None) {
        Ok(d) => {
            TRAMPOLIN_APP.store(d.trampolin as usize, Ordering::SeqCst);
            log::info(&format!(
                "detour instalado sobre AppManager::process(): parche de {} bytes, trampolin en 0x{:x}",
                d.estilo.bytes(),
                d.trampolin
            ));
        }
        Err(e) => {
            log::error(&format!("no se pudo instalar el detour de AppManager::process(): {e}"));
        }
    }
}

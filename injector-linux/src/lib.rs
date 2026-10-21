//! Inyector LD_PRELOAD para Factorio 2.1.22 ARM64 (Linux).
//!
//! Se carga con `LD_PRELOAD=/ruta/libxfactorvr_injector.so` y al arrancar:
//!
//!   1. Localiza el ejecutable principal (dl_iterate_phdr + /proc/self/exe).
//!   2. Lee su `.symtab` (el motor no esta stripeado: 148.802 simbolos C++).
//!   3. Resuelve `GameRenderer::render` por nombre des-mangled.
//!   4. Parchea la funcion con un detour ARM64 (ver el modulo `detour`).
//!   5. Cuenta los frames y, cada N, escribe "hook vivo: N frames" por stderr.
//!
//! Variable de entorno:
//!
//!   XFACTORVR_HOOK_DEBUG=0   desactiva el "pull" de depuracion (por defecto activo)
//!   XFACTORVR_HOOK_EVERY=N   cada cuantos frames se imprime (por defecto 60)
//!   XFACTORVR_HOOK_TARGET=X  otro simbolo que enganchar (por defecto GameRenderer::render)
//!   XFACTORVR_HOOK_PATCH=corto|largo  fuerza el estilo de parche
//!
//! Solo std: ni una dependencia externa.

pub mod autostart;
pub mod demangle;
pub mod detour;
pub mod diagnostico;
pub mod elf;
pub mod fase_juego;
pub mod gnu_unwind;
pub mod hooks;
pub mod log;
pub mod maps;
pub mod modelo3d;
pub mod resolver;
pub mod stream;

/// Version del inyector (por si hay que distinguir .so en el campo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Punto de entrada manual: se puede llamar con `dlsym` si algien lo
/// necesita, pero normalmente lo invoca el constructor de `.init_array`.
#[no_mangle]
pub extern "C" fn xfactorvr_injector_init() {
    // El .so se preloadea en TODOS los procesos que hereden el entorno
    // (grep, tail, python, shells...). Si el inyector hiciera su trabajo en
    // cualquiera, ensuciaria la salida de todas las herramientas y podria
    // tocar memoria de procesos ajenos. Solo se activa en el juego.
    if !es_el_juego() {
        return;
    }
    hooks::instalar();
}

/// ¿El proceso actual es el ejecutable de Factorio?
fn es_el_juego() -> bool {
    if std::env::var("XFACTORVR_FORZAR").is_ok() {
        return true;
    }
    let exe = match std::fs::read_link("/proc/self/exe") {
        Ok(p) => p,
        Err(_) => return false,
    };
    let nombre = exe
        .file_name()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    nombre == "factorio" || nombre.starts_with("factorio-")
}

/// Version numerica, por si el contenedor la consulta.
#[no_mangle]
pub extern "C" fn xfactorvr_injector_version() -> u32 {
    1
}

/// Constructor: se ejecuta al cargar la libreria (LD_PRELOAD).
///
/// Se usa `.init_array` en vez de un atributo de constructor del GCC para no
/// depender de una extension del compilador: es la forma estandar y portable
/// que usan las .so de C.
#[cfg(elf_target)]
#[used]
#[link_section = ".init_array"]
static CONSTRUCTOR: extern "C" fn() = xfactorvr_injector_init;

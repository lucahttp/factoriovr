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
pub mod elf;
pub mod gnu_unwind;
pub mod hooks;
pub mod log;
pub mod maps;
pub mod resolver;

/// Version del inyector (por si hay que distinguir .so en el campo).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// Punto de entrada manual: se puede llamar con `dlsym` si algien lo
/// necesita, pero normalmente lo invoca el constructor de `.init_array`.
#[no_mangle]
pub extern "C" fn xfactorvr_injector_init() {
    hooks::instalar();
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

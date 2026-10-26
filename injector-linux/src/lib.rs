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
//! # Modelos 3D
//!
//!   XFACTORVR_MODELOS_DIR=RUTA   directorio con `manifest.json` y los `.glb`
//!                                (los mismos ficheros que
//!                                `android/src/main/assets/models/`).  Es la
//!                                variable que dice al inyector donde estan
//!                                los modelos 3D de verdad.
//!
//!   XFACTORVR_3D=0        apaga el modulo 3D entero (no se instala ningun hook)
//!   XFACTORVR_3D_CUBO=1   dibuja tambien el cubo de prueba (ademas de los modelos)
//!   XFACTORVR_3D_SIN_SPRITE=1  el modelo 3D sustituye al sprite 2D de la entidad
//!   XFACTORVR_3D_SIN_MODELOS=1  no busca modelos (solo el cubo)
//!   XFACTORVR_3D_CADA_FLUSH=N   cada cuantos flush se redibujan los modelos
//!   XFACTORVR_3D_CAPTURA=RUTA   guarda un PNG del framebuffer del mundo
//!   XFACTORVR_3D_CAMARA=D   desplazamiento de la camara (estereo, en pixels)
//!
//! Solo std: ni una dependencia externa.

pub mod autostart;
pub mod demangle;
pub mod detour;
pub mod consola;
pub mod diagnostico;
pub mod elf;
pub mod entrada;
pub mod fase_juego;
pub mod glb;
pub mod gnu_unwind;
pub mod hooks;
pub mod log;
pub mod lua_bridge;
pub mod json;
pub mod maps;
pub mod modelo3d;
pub mod modelos;
pub mod proto;
pub mod render3d;
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
    //
    // OJO CON ANDROID: el motor de Factorio es un binario de glibc y Android
    // usa bionic, asi que en el visor hay que arrancarlo con el linker de
    // glibc ("ld-linux-aarch64.so.1 --library-path ... factorio").  En ese
    // caso, cuando corre este constructor, el motor AUN NO ESTA CARGADO: lo
    // carga el linker DESPUES de inicializar los preloads.  Por eso no basta
    // con mirar /proc/self/exe (apunta al LINKER) ni con buscar el motor en
    // la lista de objetos: hay que ESPERAR a que aparezca.
    if crate::resolver::hay_motor_cargado() {
        hooks::instalar();
        return;
    }
    if std::env::var("XFACTORVR_FORZAR").is_ok() {
        hooks::instalar();
        return;
    }
    let nombre = std::fs::read_link("/proc/self/exe")
        .ok()
        .and_then(|p| p.file_name().map(|s| s.to_string_lossy().to_string()))
        .unwrap_or_default();
    if nombre == "factorio" || nombre.starts_with("factorio-") {
        hooks::instalar();
        return;
    }
    // Proceso ajeno: no se toca nada.  Salvo que sea el LINKER de glibc, en
    // cuyo caso el motor esta a punto de aparecer.
    if nombre.starts_with("ld-linux") || nombre.starts_with("ld-") {
        esperar_al_motor();
    }
}

/// Espera a que el motor aparezca en la lista de objetos cargados.
///
/// El motor tarda una fraccion de segundo en cargarse (el linker lo mapea
/// justo despues de inicializar los preloads) y en llegar al menu pasan un
/// par de segundos, asi que da tiempo de sobra a enganchar los hooks antes
/// de que el motor los pueda estar ejecutando.
fn esperar_al_motor() {
    std::thread::spawn(|| {
        for _ in 0..1200 {
            std::thread::sleep(std::time::Duration::from_millis(5));
            if crate::resolver::hay_motor_cargado() {
                crate::log::info("el motor ha aparecido: instalo los hooks");
                hooks::instalar();
                return;
            }
        }
        crate::log::error("el motor no aparecio en 6 s: no instalo los hooks");
    });
}

/// ¿El proceso actual es el ejecutable de Factorio?
///
/// OJO: en Android el motor NO es el ejecutable principal.  El motor es un
/// binario de glibc y Android usa bionic, asi que hay que arrancarlo con el
/// linker de glibc ("ld-linux-aarch64.so.1 --library-path ... factorio").  En
/// ese caso `/proc/self/exe` es el LINKER y el motor aparece como un objeto
/// mas de los cargados: hay que mirar la lista, no solo el ejecutable.
fn es_el_juego() -> bool {
    if std::env::var("XFACTORVR_FORZAR").is_ok() {
        return true;
    }
    if crate::resolver::hay_motor_cargado() {
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

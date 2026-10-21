//! Stubs del desenrollador de excepciones de glibc.
//!
//! La `std` de Rust para `aarch64-unknown-linux-gnu` referencia los simbolos
//! `_Unwind_*` del desenrollador (los usa el soporte de backtrace y el
//! runtime de panics). El motor de Factorio los lleva ENLAZADOS ESTATICAMENTE
//! y no los exporta, y ademas no carga `libgcc_s.so.1` (sus `DT_NEEDED` son
//! solo libresolv, libm y libc). Si nuestro .so deja esos simbolos sin
//! resolver, el cargador dinamico falla al cargarlo:
//!
//! ```text
//! ./factorio: symbol lookup error: /injector.so: undefined symbol: _Unwind_Resume
//! ```
//!
//! Como el inyector se compila con `panic = "abort"`, nunca hay desenrollado:
//! un panic aborta el proceso directamente. Estos stubs existen solo para que
//! el .so pueda cargarse; si alguno se llamara, el proceso terminaria con
//! codigo 134 (como un abort) para que el fallo fuera ruidoso.
//!
//! Se emiten con `global_asm!` y visibilidad `.hidden` a proposito: si se
//! exportaran, interpondrian el desenrollador del juego para cualquier otra
//! libreria del proceso. Con `#[no_mangle]` en Rust no se puede (el version
//! script que genera Rust mete los `#[no_mangle]` en `global:`), de ahi el
//! ensamblador.
//!
//! Solo hacen falta en el target **glibc** (Linux). En Android el NDK ya
//! enlaza su propio `libunwind` (y defininglos aqui daria simbolos
//! duplicados), y en macOS el desenrollador tiene otros nombres.

#[cfg(target_os = "linux")]
use std::os::raw::c_void;
#[cfg(target_os = "linux")]
use std::sync::atomic::AtomicPtr;

/// Nombre del stub real al que apuntan todos los alias.
#[cfg(target_os = "linux")]
const IMPLEMENTACION: &str = "_Unwind_stub_impl";

/// Referencia a `c_void`: documenta la firma de los stubs y evita que el
/// modulo parezca vacio.
#[cfg(target_os = "linux")]
#[allow(dead_code)]
pub type StubArg = *mut c_void;

#[cfg(target_os = "linux")]
core::arch::global_asm!(
    // --- implementacion comun: termina el proceso como un abort ---
    ".globl _Unwind_stub_impl",
    ".type _Unwind_stub_impl, %function",
    "_Unwind_stub_impl:",
    "  mov x8, 94", // __NR_exit_group (aarch64)
    "  mov x0, 134", // 128 + SIGABRT
    "  svc #0",
    ".size _Unwind_stub_impl, .-_Unwind_stub_impl",

    // --- alias: todos los simbolos que pide la std ---
    ".globl _Unwind_GetCFA",
    ".set _Unwind_GetCFA, _Unwind_stub_impl",
    ".globl _Unwind_FindEnclosingFunction",
    ".set _Unwind_FindEnclosingFunction, _Unwind_stub_impl",
    ".globl _Unwind_Resume",
    ".set _Unwind_Resume, _Unwind_stub_impl",
    ".globl _Unwind_RaiseException",
    ".set _Unwind_RaiseException, _Unwind_stub_impl",
    ".globl _Unwind_DeleteException",
    ".set _Unwind_DeleteException, _Unwind_stub_impl",
    ".globl _Unwind_Backtrace",
    ".set _Unwind_Backtrace, _Unwind_stub_impl",
    ".globl _Unwind_GetRegionStart",
    ".set _Unwind_GetRegionStart, _Unwind_stub_impl",
    ".globl _Unwind_GetTextRelBase",
    ".set _Unwind_GetTextRelBase, _Unwind_stub_impl",
    ".globl _Unwind_GetDataRelBase",
    ".set _Unwind_GetDataRelBase, _Unwind_stub_impl",
    ".globl _Unwind_GetIP",
    ".set _Unwind_GetIP, _Unwind_stub_impl",
    ".globl _Unwind_GetIPInfo",
    ".set _Unwind_GetIPInfo, _Unwind_stub_impl",
    ".globl _Unwind_SetIP",
    ".set _Unwind_SetIP, _Unwind_stub_impl",
    ".globl _Unwind_SetGR",
    ".set _Unwind_SetGR, _Unwind_stub_impl",
    ".globl _Unwind_GetLanguageSpecificData",
    ".set _Unwind_GetLanguageSpecificData, _Unwind_stub_impl",
    ".globl __gxx_personality_v0",
    ".set __gxx_personality_v0, _Unwind_stub_impl",
);

/// Nombre de la implementacion comun (para diagnostico).
#[cfg(target_os = "linux")]
pub const IMPLEMENTACION_STUB: &str = IMPLEMENTACION;

#[cfg(target_os = "linux")]
// Declaraciones de los simbolos que define el ensamblador. Solo sirven para
// generar referencias: asi el enlazador los resuelve contra los alias
// locales (y `--gc-sections` no se lleva por delante la seccion, que si no se
// queda sin definir y los "_Unwind_*" siguen sin resolver).
#[cfg(target_os = "linux")]
extern "C" {
    fn _Unwind_GetCFA(_ctx: *mut c_void) -> usize;
    fn _Unwind_FindEnclosingFunction(_ctx: *mut c_void) -> usize;
    fn _Unwind_Resume() -> !;
    fn _Unwind_RaiseException(_exc: *mut c_void) -> !;
    fn _Unwind_DeleteException(_exc: *mut c_void);
    fn _Unwind_Backtrace(_trace: *mut c_void, _arg: *mut c_void) -> i32;
    fn _Unwind_GetRegionStart(_ctx: *mut c_void) -> usize;
    fn _Unwind_GetTextRelBase(_ctx: *mut c_void) -> usize;
    fn _Unwind_GetDataRelBase(_ctx: *mut c_void) -> usize;
    fn _Unwind_GetIP(_ctx: *mut c_void) -> usize;
    fn _Unwind_GetIPInfo(_ctx: *mut c_void, _before: *mut i32) -> usize;
    fn _Unwind_SetIP(_ctx: *mut c_void, _value: usize);
    fn _Unwind_SetGR(_ctx: *mut c_void, _index: i32, _value: usize);
    fn _Unwind_GetLanguageSpecificData(_ctx: *mut c_void) -> *mut c_void;
    fn __gxx_personality_v0(
        _version: i32,
        _actions: *mut c_void,
        _exception_class: u64,
        _exception_object: *mut c_void,
        _context: *mut c_void,
    ) -> i32;
}

/// Tabla que fuerza a que los stubs se queden en el binario.
#[cfg(target_os = "linux")]
#[used]
static STUBS: [AtomicPtr<()>; 15] = [
    AtomicPtr::new(_Unwind_GetCFA as *mut ()),
    AtomicPtr::new(_Unwind_FindEnclosingFunction as *mut ()),
    AtomicPtr::new(_Unwind_Resume as *mut ()),
    AtomicPtr::new(_Unwind_RaiseException as *mut ()),
    AtomicPtr::new(_Unwind_DeleteException as *mut ()),
    AtomicPtr::new(_Unwind_Backtrace as *mut ()),
    AtomicPtr::new(_Unwind_GetRegionStart as *mut ()),
    AtomicPtr::new(_Unwind_GetTextRelBase as *mut ()),
    AtomicPtr::new(_Unwind_GetDataRelBase as *mut ()),
    AtomicPtr::new(_Unwind_GetIP as *mut ()),
    AtomicPtr::new(_Unwind_GetIPInfo as *mut ()),
    AtomicPtr::new(_Unwind_SetIP as *mut ()),
    AtomicPtr::new(_Unwind_SetGR as *mut ()),
    AtomicPtr::new(_Unwind_GetLanguageSpecificData as *mut ()),
    AtomicPtr::new(__gxx_personality_v0 as *mut ()),
];

//! Salida de diagnostico por stderr.
//!
//! El juego corre con su propia salida por stderr; el inyector escribe
//! lineas prefijadas con `[xfv-injector]` para poder filtrarlas.
//!
//! IMPORTANTE: no se usa `std::io::stderr()`. En el target
//! `aarch64-unknown-linux-gnu` la std de Rust saca el puntero a `FILE` de
//! stderr del simbolo interno de glibc `__sF`, que en aarch64 **no** esta
//! exportado: el .so no cargaria ("undefined symbol: __sF"). Se escribe
//! directo con `write(2)`, que si exporta glibc en todas las arquitecturas.
//!
//! Ninguna escritura puede fallar con panic: si stderr no esta disponible,
//! se ignora el error.

use std::os::raw::{c_int, c_void};

/// Prefijo de todas las lineas del inyector.
const PREFIJO: &str = "[xfv-injector] ";

extern "C" {
    /// `write(2)`: escribe en el descriptor de fichero indicado.
    fn write(fd: c_int, buf: *const c_void, count: usize) -> isize;
}

/// Escribe bytes en stderr (fd 2) con `write(2)`.
pub fn escribir_stderr(bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    // SAFETY: `bytes` es un slice valido y `write` solo lee `count` bytes.
    unsafe {
        let _ = write(2, bytes.as_ptr() as *const c_void, bytes.len());
    }
}

/// Escribe una linea de informacion.
pub fn info(msg: &str) {
    escribir("info", msg);
}

/// Escribe una linea de aviso.
pub fn aviso(msg: &str) {
    escribir("aviso", msg);
}

/// Escribe una linea de error.
pub fn error(msg: &str) {
    escribir("error", msg);
}

fn escribir(nivel: &str, msg: &str) {
    // `format!` no toca stdio (no usa el allocator de la std para E/S).
    let linea = format!("{PREFIJO}{nivel}: {msg}\n");
    escribir_stderr(linea.as_bytes());
}

/// Vuelca `bytes` en hexadecimal por stderr (diagnostico), 16 bytes por linea.
pub fn hexdump(bytes: &[u8]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut linea = [0u8; 16 * 3];
    for (n, b) in bytes.iter().enumerate() {
        let i = (n % 16) * 3;
        linea[i] = HEX[(b >> 4) as usize];
        linea[i + 1] = HEX[(b & 0xf) as usize];
        linea[i + 2] = b' ';
        if n % 16 == 15 {
            crate::log::escribir_stderr(&linea);
            crate::log::escribir_stderr(b"\n");
        }
    }
    let resto = bytes.len() % 16;
    if resto != 0 {
        crate::log::escribir_stderr(&linea[..resto * 3]);
        crate::log::escribir_stderr(b"\n");
    }
}

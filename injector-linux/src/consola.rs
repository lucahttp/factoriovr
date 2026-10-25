//! consola.rs — Mandar ordenes al motor por su CONSOLA (sin RCON).
//!
//! # Por que
//!
//! Un mod de Factorio no puede abrir puertos (su sandbox de Lua no tiene
//! sockets ni io), pero el motor **lee ordenes por su stdin**: es la consola
//! de Factorio, y por ahi se puede ejecutar cualquier comando, incluido un
//! `remote.call` a un mod del servidor.
//!
//! Eso da el canal de ACCIONES sin RCON (ADR-020):
//!
//! ```text
//!   visor (APK)  ──escribe──►  /sdcard/XFactorVR/consola/pendiente-*.txt
//!                                        │
//!                                        ▼ (el inyector lo vigila)
//!                                  motor: stdin (fd 0)
//!                                        │
//!                                        ▼
//!                                  consola del motor -> remote.call -> mod
//! ```
//!
//! El visor escribe en `/sdcard` porque es lo unico que puede leer con el
//! almacenamiento con ambito (SELinux le impide tocar /data/local/tmp).  El
//! inyector, que corre como `shell`, si puede.
//!
//! # Por que el motor tiene que leer de una FIFO
//!
//! Si el motor arranca con stdin a EOF (o a /dev/null) cierra el lector de
//! la consola y ya no acepta ni una orden mas ("Got EOF on stdin; closing").
//! Por eso el launcher le pasa una FIFO abierta en lectura-escritura: nunca
//! hay EOF y el inyector puede escribir en el fd 0 del propio motor.

use crate::log;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Variable de entorno con el directorio donde el visor deja las ordenes.
const ENV_DIR: &str = "XFACTORVR_CONSOLA_DIR";

/// Como se llama una orden pendiente: `pendiente-<n>.txt`.
///
/// El numero evita que dos ordenes simultaneas se pisen, y el visor puede
/// vaciar el directorio cuando ya no le haga falta.
fn nombre_pendiente(n: u64) -> String {
    format!("pendiente-{n:08}.txt")
}

/// Escribe una orden en la consola del motor (su fd 0).
///
/// Devuelve false si no se pudo escribir.
pub fn escribir_en_la_consola(orden: &str) -> bool {
    let mut linea = String::with_capacity(orden.len() + 2);
    linea.push_str(orden);
    if !linea.ends_with('\n') {
        linea.push('\n');
    }
    // SAFETY: escribir en el fd 0 (la consola del motor).  Si el motor ya
    // cerro el lector, la escritura falla y no pasa nada: por eso se
    // comprueba el resultado.
    let n = unsafe {
        libc_write(
            0,
            linea.as_ptr() as *const u8,
            linea.len(),
        )
    };
    n == linea.len() as isize
}

/// Escribe en un descriptor de fichero (envuelto para no depender de libc).
unsafe fn libc_write(fd: i32, buf: *const u8, len: usize) -> isize {
    // El simbolo `write` de la libc se resuelve en tiempo de ejecucion: el
    // inyector se compila para Android (bionic) y para glibc, y `write` tiene
    // el mismo nombre en las dos.
    extern "C" {
        fn write(fd: i32, buf: *const u8, count: usize) -> isize;
    }
    unsafe { write(fd, buf, len) }
}

/// El directorio donde el visor deja las ordenes (si esta configurado).
pub fn directorio() -> Option<PathBuf> {
    match std::env::var(ENV_DIR) {
        Ok(d) if !d.trim().is_empty() => Some(PathBuf::from(d)),
        _ => None,
    }
}

/// Vigila el directorio de ordenes y las pasa a la consola del motor.
///
/// Se queda en un hilo propio: el motor dibuja en varios y no puede
/// quedarse esperando.
pub fn vigilar() {
    let Some(dir) = directorio() else {
        return;
    };
    if std::fs::create_dir_all(&dir).is_err() {
        log::aviso(&format!("consola: no se pudo crear {}", dir.display()));
        return;
    }
    log::info(&format!(
        "consola: vigilando las ordenes del visor en {}",
        dir.display()
    ));

    let mut ya_vistos: Vec<String> = Vec::new();
    loop {
        std::thread::sleep(std::time::Duration::from_millis(120));
        let Ok(entradas) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entrada in entradas.flatten() {
            let nombre = entrada.file_name().to_string_lossy().to_string();
            if !nombre.starts_with("pendiente-") || ya_vistos.iter().any(|v| *v == nombre) {
                continue;
            }
            ya_vistos.push(nombre.clone());
            let Ok(texto) = std::fs::read_to_string(entrada.path()) else {
                continue;
            };
            for linea in texto.lines() {
                let linea = linea.trim();
                if linea.is_empty() {
                    continue;
                }
                if escribir_en_la_consola(linea) {
                    log::info(&format!("consola: orden enviada ({})", linea));
                } else {
                    log::aviso("consola: el motor no acepto la orden (stdin cerrado?)");
                }
            }
            // El fichero ya se ha vaciado: se puede borrar.
            let _ = std::fs::remove_file(entrada.path());
        }
        // La lista de vistos no tiene que crecer sin limite.
        if ya_vistos.len() > 256 {
            ya_vistos.drain(0..128);
        }
    }
}

/// Instala el vigilante de la consola.
pub fn instalar() {
    if directorio().is_none() {
        return;
    }
    std::thread::Builder::new()
        .name("xfv-consola".to_string())
        .spawn(vigilar)
        .ok();
}

/// El nombre del fichero que le tocaria al visor para la orden `n`.
///
/// Lo usa el banco y sirve de contrato con el lado del visor.
pub fn nombre_de_orden(n: u64) -> String {
    nombre_pendiente(n)
}

/// Escribe una orden en un directorio (lo que hace el visor).
///
/// Se separa del vigilante para poder probarlo sin threads ni motores.
pub fn escribir_orden(dir: &Path, n: u64, orden: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let ruta = dir.join(nombre_pendiente(n));
    let mut f = std::fs::File::create(&ruta)?;
    f.write_all(orden.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(ruta)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// El visor deja la orden en un fichero con el nombre que espera el
    /// inyector.
    #[test]
    fn la_orden_se_escribe_con_el_nombre_que_espera_el_inyector() {
        let dir = std::env::temp_dir().join(format!("xfv_consola_{}", std::process::id()));
        let ruta = escribir_orden(&dir, 7, "/silent-command remote.call('xfactorvr', 'x')")
            .expect("no se pudo escribir");
        let nombre = ruta.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(nombre, "pendiente-00000007.txt");
        let texto = std::fs::read_to_string(&ruta).expect("no se pudo leer");
        assert!(texto.contains("remote.call"), "la orden tiene que ir entera");
    }

    /// Escribir en un fd que no existe no puede reventar el motor.
    #[test]
    fn escribir_en_un_fd_invalido_no_rompe() {
        // El fd 99 no esta abierto: la escritura falla y se devuelve false.
        let mut buf = [0u8; 4];
        let n = unsafe { libc_write(99, buf.as_mut_ptr(), buf.len()) };
        assert!(n < 0, "tiene que fallar");
    }
}

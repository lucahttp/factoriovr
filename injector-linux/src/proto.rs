//! Descubrir el **prototipo** de una entidad (para saber que modelo 3D le
//! toca).
//!
//! # Por que hace falta
//!
//! Una clase del motor puede tener varios prototipos: `AssemblingMachine` es
//! la ensambladora 1, la 2 y la 3 (tres modelos distintos).  Para saber cual
//! hay que mirar el **prototipo** de la entidad, que es un `EntityPrototype*`
//! con un `std::string` con el nombre ("assembling-machine-1").
//!
//! En la build 2.1.22 de ARM64 **no existe** `Entity::getPrototype()` (en la
//! build de Windows del mod si), asi que el puntero al prototipo se localiza
//! **en caliente**: se busca, dentro del objeto de la entidad, un puntero que
//! apunte a un objeto que tenga una cadena con un nombre de prototipo
//! conocido.  El par (desplazamiento en la entidad, desplazamiento en el
//! prototipo) se cachea por clase, asi que solo se busca una vez.
//!
//! # El `std::string` de libstdc++
//!
//! En el ABI nuevo de libstdc++ (el de este motor) un `std::string` es:
//!
//! ```text
//!   +0x00  puntero a los datos (o NULL si es SSO)
//!   +0x08  longitud
//!   +0x10  capacidad (15 = SSO: los datos estan en +0x10, en linea)
//! ```
//!
//! En el ABI viejo (COW) los tres campos son iguales pero los datos siempre
//! estan en el puntero.  Se couvren los dos.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use crate::log;
use crate::modelos;

/// Desplazamiento en la entidad del puntero al prototipo (por clase).
#[derive(Clone, Copy, Debug)]
struct OffsetClase {
    /// Desplazamiento del puntero dentro de la entidad.
    entidad: usize,
    /// Desplazamiento de la cadena del nombre dentro del prototipo.
    prototipo: usize,
}

/// Cache de los offsets por clase de entidad.
static OFFSETS: OnceLock<Mutex<HashMap<usize, Option<OffsetClase>>>> = OnceLock::new();

/// Cache puntero de prototipo -> nombre del prototipo.
static NOMBRES: OnceLock<Mutex<HashMap<usize, Option<String>>>> = OnceLock::new();

/// Cuantos prototipos se han identificado (para el log).
pub static IDENTIFICADOS: AtomicU64 = AtomicU64::new(0);

/// Nombres de prototipo conocidos (los del manifiesto de modelos).
fn nombres_conocidos() -> &'static Vec<String> {
    static N: OnceLock<Vec<String>> = OnceLock::new();
    N.get_or_init(|| {
        let mut v = Vec::new();
        if let Some(dir) = modelos::dir_modelos() {
            if let Some(m) = modelos::cargar_manifesto(&dir) {
                for info in &m.modelos {
                    v.push(info.prototipo.clone());
                }
            }
        }
        v
    })
}

/// Nombre del prototipo de una entidad, si se puede.
///
/// `clase` es el indice de la clase (el de la lista de hooks).
pub fn nombre_de_prototipo(this: *mut core::ffi::c_void, clase: usize) -> Option<String> {
    if this.is_null() {
        return None;
    }
    let conocidos = nombres_conocidos();
    if conocidos.is_empty() {
        return None;
    }
    let offsets = OFFSETS.get_or_init(|| Mutex::new(HashMap::new()));
    let off = {
        let mut g = offsets.lock().unwrap_or_else(|p| p.into_inner());
        *g.entry(clase).or_insert_with(|| buscar_offsets(this, conocidos))
    }?;

    // El puntero al prototipo, en la entidad.
    let proto = unsafe { std::ptr::read_unaligned((this as usize + off.entidad) as *const usize) };
    if proto == 0 {
        return None;
    }
    nombre_en_prototipo(proto, off.prototipo, conocidos)
}

/// El nombre del prototipo, a partir del puntero al prototipo.
fn nombre_en_prototipo(proto: usize, off: usize, conocidos: &[String]) -> Option<String> {
    let cache = NOMBRES.get_or_init(|| Mutex::new(HashMap::new()));
    {
        let g = cache.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(v) = g.get(&proto) {
            return v.clone();
        }
    }
    let nombre = cadena_en(proto + off).and_then(|s| {
        if conocidos.iter().any(|k| k == &s) {
            Some(s)
        } else {
            None
        }
    });
    {
        let mut g = cache.lock().unwrap_or_else(|p| p.into_inner());
        g.insert(proto, nombre.clone());
    }
    if let Some(nombre) = nombre.as_ref() {
        let n = IDENTIFICADOS.fetch_add(1, Ordering::Relaxed) + 1;
        if n <= 16 {
            log::info(&format!("prototipo identificado: {nombre}"));
        }
    }
    nombre
}

/// Busca los offsets (entidad, prototipo) leyendo la entidad.
///
/// Se prueban los punteros de la entidad (de 8 en 8 bytes) y, para cada uno,
/// se busca una cadena con un nombre de prototipo conocido.
fn buscar_offsets(this: *mut core::ffi::c_void, conocidos: &[String]) -> Option<OffsetClase> {
    let base = this as usize;
    for eoff in (0..0x300usize).step_by(8) {
        // La entidad puede ser mas pequeña que 0x300 bytes: no se lee fuera
        // de su memoria (si no, el motor se cae con SIGSEGV).
        if !legible(base + eoff, 8) {
            break;
        }
        let p = unsafe { std::ptr::read_unaligned((base + eoff) as *const usize) };
        if p < 0x1000 || p & 7 != 0 {
            continue;
        }
        if !legible(p, 8) {
            continue;
        }
        for poff in (0..0x300usize).step_by(8) {
            if !legible(p + poff, 32) {
                continue;
            }
            let s = match cadena_en(p + poff) {
                Some(s) => s,
                None => continue,
            };
            if conocidos.iter().any(|k| k == &s) {
                return Some(OffsetClase {
                    entidad: eoff,
                    prototipo: poff,
                });
            }
        }
    }
    None
}

/// Lee un `std::string` de libstdc++ en la direccion dada.
///
/// Devuelve el texto si parece una cadena razonable.
fn cadena_en(dir: usize) -> Option<String> {
    if !legible(dir, 24) {
        return None;
    }
    let puntero = unsafe { std::ptr::read_unaligned(dir as *const usize) };
    let largo = unsafe { std::ptr::read_unaligned((dir + 8) as *const usize) };
    let capacidad = unsafe { std::ptr::read_unaligned((dir + 16) as *const usize) };
    if !(3..=64).contains(&largo) {
        return None;
    }
    // En el ABI nuevo de libstdc++, cuando la cadena es corta (SSO) el
    // puntero apunta a la propia cadena (dir + 16) y los bytes de +16 son
    // los CARACTERES (no la capacidad).  Con la cadena en el heap, el
    // puntero apunta a los datos y +16 es la capacidad.
    let datos: *const u8 = if puntero == dir + 16 {
        if largo > 15 {
            return None;
        }
        (dir + 16) as *const u8
    } else {
        if puntero == 0 || !legible(puntero, largo as usize + 1) {
            return None;
        }
        if capacidad < largo || capacidad >= 0x10000 {
            return None;
        }
        puntero as *const u8
    };
    let bytes = unsafe { std::slice::from_raw_parts(datos, largo as usize) };
    if !bytes.iter().all(|&b| (0x20..0x7f).contains(&b)) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

// --------------------------------------------------------------------------
// Memoria legible
// --------------------------------------------------------------------------

/// Rangos de memoria legible del proceso (de `/proc/self/maps`).
struct Rangos {
    /// Inicio y fin de cada region legible.
    regiones: Vec<(usize, usize)>,
    /// Cuando se leyeron (para refrescar de vez en cuando).
    leido: u64,
}

/// Cuanto se cachean los rangos (en nanosegundos): medio minuto.
const TTL_NANOS: u64 = 30_000_000_000;

static RANGOS: OnceLock<Mutex<Rangos>> = OnceLock::new();

/// ¿Está la memoria `[dir, dir+len)` comprometida y legible?
pub fn legible(dir: usize, len: usize) -> bool {
    if dir == 0 || len == 0 {
        return false;
    }
    // En una plataforma sin `/proc` (el host de desarrollo) no hay forma de
    // preguntar: se asume que si se puede leer.
    if !std::path::Path::new("/proc/self/maps").exists() {
        return true;
    }
    let cache = RANGOS.get_or_init(|| Mutex::new(Rangos { regiones: Vec::new(), leido: 0 }));
    let ahora =ahora_ns();
    {
        let g = cache.lock().unwrap_or_else(|p| p.into_inner());
        if ahora.saturating_sub(g.leido) < TTL_NANOS && !g.regiones.is_empty() {
            return dentro(&g.regiones, dir, len);
        }
    }
    let regiones = leer_maps();
    let mut g = cache.lock().unwrap_or_else(|p| p.into_inner());
    g.regiones = regiones;
    g.leido = ahora;
    dentro(&g.regiones, dir, len)
}

/// ¿El rango esta dentro de alguna region legible?
fn dentro(regiones: &[(usize, usize)], dir: usize, len: usize) -> bool {
    let fin = dir.saturating_add(len);
    regiones
        .iter()
        .any(|(a, b)| dir >= *a && fin <= *b)
}

/// Lee `/proc/self/maps` y saca las regiones legibles.
fn leer_maps() -> Vec<(usize, usize)> {
    let texto = match std::fs::read_to_string("/proc/self/maps") {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };
    let mut out = Vec::new();
    for linea in texto.lines() {
        // "rwxr-xr-x 00000000 0000:000 00000000 00000000 00:00 0 [heap]"
        let mut partes = linea.split_whitespace();
        let rango = match partes.next() {
            Some(r) => r,
            None => continue,
        };
        let perms = match partes.next() {
            Some(p) => p,
            None => continue,
        };
        if !perms.starts_with('r') {
            continue;
        }
        let mut trozos = rango.split('-');
        let ini = match trozos.next().and_then(|h| u64::from_str_radix(h, 16).ok()) {
            Some(v) => v as usize,
            None => continue,
        };
        let fin = match trozos.next().and_then(|h| u64::from_str_radix(h, 16).ok()) {
            Some(v) => v as usize,
            None => continue,
        };
        out.push((ini, fin));
    }
    out
}

/// Reloj monotono (nanosegundos).
fn ahora_ns() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lee_un_string_de_libstdcxx() {
        // Un std::string con SSO: la longitud en +8 y la capacidad en +16.
        let mut buf = [0u8; 64];
        let texto = b"stone-furnace";
        let dir = buf.as_mut_ptr() as usize;
        buf[8] = texto.len() as u8;
        // Con SSO el puntero apunta a la propia cadena (+16) y ahi van los
        // caracteres.
        buf[0..8].copy_from_slice(&((dir + 16) as u64).to_le_bytes());
        buf[16..16 + texto.len()].copy_from_slice(texto);
        let s = cadena_en(dir).expect("no se leyo el string");
        assert_eq!(s, "stone-furnace");
    }

    #[test]
    fn lee_un_string_en_la_pila() {
        // Un std::string con los datos en el heap (capacidad > 15).
        let datos = b"assembling-machine-1".to_vec();
        let mut buf = [0u8; 32];
        buf[0..8].copy_from_slice(&(datos.as_ptr() as u64).to_le_bytes());
        buf[8] = datos.len() as u8;
        buf[16] = 32; // capacidad
        let s = cadena_en(buf.as_ptr() as usize).expect("no se leyo el string");
        assert_eq!(s, "assembling-machine-1");
    }

    #[test]
    fn lo_que_no_es_un_string_no_entra() {
        let mut buf = [0u8; 64];
        // longitud imposible
        buf[8] = 200;
        assert!(cadena_en(buf.as_ptr() as usize).is_none());
        // longitud 0
        let mut buf2 = [0u8; 64];
        buf2[16] = 15;
        assert!(cadena_en(buf2.as_ptr() as usize).is_none());
    }

    #[test]
    fn la_memoria_ilegible_se_detecta() {
        assert!(!legible(0, 1));
        // La comprobacion de paginas es solo de Linux (en el host de
        // desarrollo no hay /proc/self/maps).
        if std::path::Path::new("/proc/self/maps").exists() {
            assert!(!legible(0x10, 1));
        }
        // La pila es legible.
        let x = 0u8;
        assert!(legible(&x as *const u8 as usize, 1));
    }
}

//! Resolucion de simbolos en tiempo de ejecucion.
//!
//! El motor de Factorio no exporta sus simbolos en la tabla dinamica (no es
//! una libreria), pero el ELF NO esta stripeado: los nombres estan en
//! `.symtab`. Este modulo:
//!
//!   1. Localiza el ejecutable principal y su direccion base con
//!      `dl_iterate_phdr` (equivalente a `/proc/self/maps` pero exacto).
//!   2. Lee su `.symtab` del fichero en disco (`/proc/self/exe`).
//!   3. Convierte el nombre mangled a "Owner::metodo" y busca la coincidencia
//!      exacta.
//!   4. Traduce la direccion virtual del simbolo a la direccion real en
//!      memoria: `load_bias + st_value`.
//!
//! `load_bias` es la direccion base con la que el loader mapeo el ejecutable;
//! se deduce de `dlpi_addr` menos el `p_vaddr` del primer PT_LOAD (0 en el
//! motor, asi que `load_bias == dlpi_addr`).

#[cfg(elf_target)]
use std::ffi::CStr;
#[cfg(elf_target)]
use std::os::raw::{c_char, c_int, c_void};
use std::path::PathBuf;

use crate::demangle;
use crate::elf::ElfImage;

/// `struct dl_phdr_info` de glibc (linux/elf.h).
#[repr(C)]
#[cfg(elf_target)]
struct DlPhdrInfo {
    dlpi_addr: usize,
    dlpi_name: *const c_char,
    dlpi_phdr: *const c_void,
    dlpi_phnum: u16,
    dlpi_adds: u64,
    dlpi_subs: u64,
    dlpi_tls_modid: usize,
    dlpi_tls_data: *mut c_void,
}

#[cfg(elf_target)]
extern "C" {
    fn dl_iterate_phdr(
        callback: extern "C" fn(*mut DlPhdrInfo, usize, *mut c_void) -> c_int,
        data: *mut c_void,
    ) -> c_int;
}

/// Datos del ejecutable principal en memoria.
pub struct Proceso {
    /// Direccion base (load bias) con la que se mapeo el ejecutable.
    pub load_bias: u64,
    /// Ruta del ejecutable en disco.
    pub ruta: PathBuf,
}

/// Localiza el ejecutable principal: base en memoria y ruta en disco.
#[cfg(elf_target)]
pub fn proceso_principal() -> Option<Proceso> {
    let mut base: u64 = 0;
    unsafe {
        extern "C" fn callback(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
            if info.is_null() || data.is_null() {
                return 1;
            }
            // SAFETY: la glibc garantiza que `info` y `data` son punteros
            // validos mientras dura la llamada.
            unsafe {
                let info = &*info;
                // La primera entrada de dl_iterate_phdr es el ejecutable
                // principal y llega con dlpi_name == "".
                if !info.dlpi_name.is_null() {
                    let nombre = CStr::from_ptr(info.dlpi_name);
                    if !nombre.to_bytes().is_empty() {
                        return 0; // librerias: seguimos buscando
                    }
                }
                std::ptr::write(data as *mut u64, info.dlpi_addr as u64);
                1 // parar: ya tenemos el principal
            }
        }
        dl_iterate_phdr(callback, &mut base as *mut u64 as *mut c_void);
    }
    if base == 0 {
        return None;
    }

    // Ruta real del ejecutable: /proc/self/exe es lo mas fiable (el fichero
    // puede haberse renombrado o BORRADO, en cuyo caso el symlink sigue
    // apuntando al inode correcto).
    let ruta = match std::fs::read_link("/proc/self/exe") {
        Ok(p) => p,
        Err(_) => return None,
    };
    Some(Proceso { load_bias: base, ruta })
}

#[cfg(not(elf_target))]
pub fn proceso_principal() -> Option<Proceso> {
    None
}

/// Simbolo resuelto.
pub struct Resuelto {
    /// Nombre tal cual aparece en el symtab (mangled).
    pub mangled: String,
    /// Nombre legible ("GameRenderer::render").
    pub legible: String,
    /// Nombre cualificado sin argumentos ("GameRenderer::render").
    pub qualified: String,
    /// Direccion real en memoria.
    pub direccion: u64,
    /// Tamano del simbolo en bytes (0 si el ELF no lo dice).
    pub tamano: u64,
}

/// Indice de simbolos del ejecutable, ya des-mangleados.
pub struct Simbolos {
    pub imagen: ElfImage,
    pub load_bias: u64,
}

impl Simbolos {
    /// Carga y parsea el `.symtab` del ejecutable indicado.
    pub fn cargar(proceso: &Proceso) -> Option<Simbolos> {
        let imagen = ElfImage::parse(&proceso.ruta).ok()?;
        Some(Simbolos { imagen, load_bias: proceso.load_bias })
    }

    /// Resuelve un simbolo por nombre cualificado ("GameRenderer::render").
    ///
    /// Se hace un pre-filtrado barato sobre el nombre mangled (las partes del
    /// nombre tienen que aparecer como subcadena) y solo se des-manglean los
    /// candidatos: des-manglear 148.802 simbolos en cada arranque es lento.
    pub fn resolver(&self, consulta: &str) -> Option<Resuelto> {
        let partes: Vec<&str> = consulta.split("::").filter(|p| !p.is_empty()).collect();
        if partes.is_empty() {
            return None;
        }

        for s in &self.imagen.symbols {
            if !partes.iter().all(|p| s.name.contains(p)) {
                continue;
            }
            let d = match demangle::demangle(&s.name) {
                Some(d) => d,
                None => continue,
            };
            if d.qualified != consulta {
                continue;
            }
            let direccion = self.load_bias.wrapping_add(s.value);
            return Some(Resuelto {
                mangled: s.name.clone(),
                legible: d.full,
                qualified: d.qualified,
                direccion,
                tamano: s.size,
            });
        }
        None
    }

    /// Resuelve un simbolo por su nombre MANGLED exacto.
    ///
    /// Es la via sin ambiguedad para los constructores de C++: el nombre
    /// des-mangled no distingue sobrecargas (tres constructores se llaman
    /// "ScenarioExecutionContext::ScenarioExecutionContext"), pero el nombre
    /// mangled si. Se usa para llamar a funciones internas del motor con la
    /// firma exacta (ver el modulo `autostart`).
    pub fn resolver_exacto(&self, mangled: &str) -> Option<Resuelto> {
        for s in &self.imagen.symbols {
            if s.name != mangled {
                continue;
            }
            let (legible, qualified) = match demangle::demangle(&s.name) {
                Some(d) => (d.full, d.qualified),
                None => (mangled.to_string(), mangled.to_string()),
            };
            let direccion = self.load_bias.wrapping_add(s.value);
            return Some(Resuelto {
                mangled: s.name.clone(),
                legible,
                qualified,
                direccion,
                tamano: s.size,
            });
        }
        None
    }

    /// Lista de nombres calificados que empiezan por `prefijo` (diagnostico).
    pub fn buscar(&self, prefijo: &str) -> Vec<String> {
        let mut out = Vec::new();
        for s in &self.imagen.symbols {
            if !s.name.contains(prefijo) {
                continue;
            }
            if let Some(d) = demangle::demangle(&s.name) {
                if d.qualified.starts_with(prefijo) && !out.contains(&d.qualified) {
                    out.push(d.qualified);
                }
            }
        }
        out.sort();
        out
    }
}

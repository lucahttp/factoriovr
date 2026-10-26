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
///
/// OJO CON ANDROID: el motor de Factorio es un binario de GLIBC y Android usa
/// bionic, asi que en el visor hay que arrancarlo con el linker de glibc:
///
///     ld-linux-aarch64.so.1 --library-path ... /data/local/tmp/factorio/bin/arm64/factorio
///
/// En ese caso el proceso principal es el LINKER, no el motor: el
/// `/proc/self/exe` apunta a `ld-linux-aarch64.so.1` y el motor aparece como
/// un objeto mas en la lista de los cargados.  Con el metodo antiguo (el
/// primer objeto sin nombre) el inyector se enganchaba al LINKER: leia 37
/// simbolos en vez de 129.219 y no encontraba ni un `GameRenderer::render`,
/// asi que en el visor el inyector no estaba vivo NUNCA.
///
/// Ahora se busca el objeto que sea el motor (su ruta acaba en "factorio")
/// y, si no hay ninguno, se cae al ejecutable principal de toda la vida.
#[cfg(elf_target)]
pub fn proceso_principal() -> Option<Proceso> {
    #[derive(Default)]
    struct Estado {
        base: usize,
        ruta: String,
        hay_principal: bool,
    }

    let mut estado = Estado::default();
    unsafe {
        extern "C" fn callback(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
            // SAFETY: la glibc garantiza que `info` y `data` son punteros
            // validos mientras dura la llamada.
            unsafe {
                if info.is_null() || data.is_null() {
                    return 1;
                }
                let info = &*info;
                let estado = &mut *(data as *mut Estado);
                let nombre = if info.dlpi_name.is_null() {
                    String::new()
                } else {
                    CStr::from_ptr(info.dlpi_name).to_string_lossy().to_string()
                };

                // 1. El motor: el objeto cuya ruta es el binario de Factorio.
                if !nombre.is_empty() && nombre.contains("factorio") && !nombre.ends_with(".so") {
                    estado.base = info.dlpi_addr as usize;
                    estado.ruta = nombre;
                    return 1; // encontrados: parar
                }

                // 2. El ejecutable principal (el motor cuando se ejecuta
                //    directamente, el linker cuando no).
                if nombre.is_empty() && !estado.hay_principal {
                    estado.hay_principal = true;
                    estado.base = info.dlpi_addr as usize;
                    estado.ruta = std::fs::read_link("/proc/self/exe")
                        .map(|p| p.to_string_lossy().to_string())
                        .unwrap_or_default();
                }
                0 // seguir buscando: puede que el motor aparezca despues
            }
        }
        dl_iterate_phdr(callback, &mut estado as *mut Estado as *mut c_void);
    }

    // El mapa de memoria manda: con el linker de glibc (Android) el motor
    // aparece en la lista de objetos con el nombre vacio, igual que el
    // linker, y solo el mapa lo distingue.
    if let Some((base, ruta)) = motor_en_mapa() {
        return Some(Proceso {
            load_bias: base,
            ruta: PathBuf::from(ruta),
        });
    }

    if estado.base == 0 {
        return None;
    }

    // La ruta puede venir relativa (si el motor se lanzo con una ruta
    // relativa): se resuelve contra el directorio actual.
    let ruta = if estado.ruta.starts_with('/') {
        PathBuf::from(estado.ruta)
    } else {
        std::fs::canonicalize(&estado.ruta).unwrap_or_else(|_| PathBuf::from(&estado.ruta))
    };

    Some(Proceso {
        load_bias: estado.base as u64,
        ruta,
    })
}

#[cfg(not(elf_target))]
pub fn proceso_principal() -> Option<Proceso> {
    None
}

/// ¿Hay cargado el binario del motor (Factorio) en este proceso?
///
/// En Android el motor NO es el ejecutable principal (se arranca con el
/// linker de glibc), asi que mirar solo `/proc/self/exe` no vale: hay que
/// mirar la lista de objetos cargados.
#[cfg(elf_target)]
pub fn hay_motor_cargado() -> bool {
    // El mapa de memoria es lo fiable (el motor aparece con su ruta).
    if motor_en_mapa().is_some() {
        return true;
    }
    // Y la lista de objetos, por si acaso.
    let mut encontrado = false;
    unsafe {
        extern "C" fn callback(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
            unsafe {
                if info.is_null() || data.is_null() {
                    return 1;
                }
                let info = &*info;
                if info.dlpi_name.is_null() {
                    return 0;
                }
                let nombre = CStr::from_ptr(info.dlpi_name).to_string_lossy().to_string();
                if !nombre.is_empty() && nombre.contains("factorio") && !nombre.ends_with(".so") {
                    *(data as *mut bool) = true;
                    return 1; // el motor esta cargado
                }
                0
            }
        }
        dl_iterate_phdr(callback, &mut encontrado as *mut bool as *mut c_void);
    }
    encontrado
}

#[cfg(not(elf_target))]
pub fn hay_motor_cargado() -> bool {
    false
}

/// Lista los objetos cargados en el proceso: "nombre@base".
///
/// Diagnostico: en Android el motor se arranca con el linker de glibc y
/// parece que el motor no esta en la lista... o si, pero con otro nombre.
#[cfg(elf_target)]
pub fn lista_cargados() -> Vec<String> {
    let mut fuera: Vec<String> = Vec::new();
    unsafe {
        extern "C" fn callback(info: *mut DlPhdrInfo, _size: usize, data: *mut c_void) -> c_int {
            unsafe {
                if info.is_null() || data.is_null() {
                    return 1;
                }
                let info = &*info;
                let lista = &mut *(data as *mut Vec<String>);
                let nombre = if info.dlpi_name.is_null() {
                    "(principal)".to_string()
                } else {
                    CStr::from_ptr(info.dlpi_name).to_string_lossy().to_string()
                };
                lista.push(format!("{}@0x{:x}", nombre, info.dlpi_addr));
                0
            }
        }
        dl_iterate_phdr(callback, &mut fuera as *mut Vec<String> as *mut c_void);
    }
    fuera
}

#[cfg(not(elf_target))]
pub fn lista_cargados() -> Vec<String> {
    Vec::new()
}

/// Busca el motor (el binario de Factorio) en el mapa de memoria del
/// proceso: devuelve (base, ruta).
///
/// Es la forma FIABLE de encontrarlo en Android: alli el motor lo carga el
/// linker de glibc como programa principal, y un programa principal siempre
/// aparece en `dl_iterate_phdr` con el nombre VACIO (igual que el linker),
/// asi que por la lista de objetos no se puede distinguir.  En el mapa de
/// memoria, en cambio, cada fichero aparece con su ruta.
#[cfg(elf_target)]
pub fn motor_en_mapa() -> Option<(u64, String)> {
    let maps = std::fs::read_to_string("/proc/self/maps").ok()?;
    let mut base: Option<u64> = None;
    let mut ruta = String::new();
    for linea in maps.lines() {
        let Some((rango, resto)) = linea.split_once(' ') else {
            continue;
        };
        let Some((ini, _fin)) = rango.split_once('-') else {
            continue;
        };
        let Ok(ini) = u64::from_str_radix(ini, 16) else {
            continue;
        };
        // perms offset dev inode [ruta...]
        let campos: Vec<&str> = resto.split_whitespace().collect();
        if campos.len() < 5 {
            continue;
        }
        let ruta_fichero = campos[4..].join(" ");
        if !ruta_fichero.contains("factorio") || ruta_fichero.ends_with(".so") {
            continue;
        }
        let offset = u64::from_str_radix(campos[1], 16).unwrap_or(0);
        let inicio = ini.saturating_sub(offset);
        base = Some(match base {
            Some(b) => b.min(inicio),
            None => inicio,
        });
        ruta = ruta_fichero;
    }
    base.map(|b| (b, ruta))
}

#[cfg(not(elf_target))]
pub fn motor_en_mapa() -> Option<(u64, String)> {
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
